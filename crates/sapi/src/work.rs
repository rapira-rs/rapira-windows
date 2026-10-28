use crossbeam_channel::Sender;
use std::ffi::CStr;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, TryAcquireError};

use crate::scoreboard::{Event, sb_update};
use crate::{zend_class_entry, zend_object};
use rapira_scoreboard::Pool;

pub(crate) const INTAKE_WAIT: Duration = Duration::from_secs(30);
const INTAKE_SLOTS: usize = 1024;
const BODY_BUDGET: usize = 64 << 20;

/// The cycle bookkeeping view of a unit that receive() handed out.
pub trait Held {
    /// The worker committed the outcome, or discarded the unit.
    fn settled(&self) -> bool;
    /// The unit takes no outcome: the client left, or the plugin closed the unit. Work::isCancelled().
    fn cancelled(&self) -> bool;
    fn discard(&mut self);

    /// Work::isFinalized().
    fn is_finalized(&self) -> bool {
        self.settled() || self.cancelled()
    }
}

/// Reclaims the Box that receive() handed out: clears the cycle slot if it still points here, and counts an unfinalized unit as handled.
/// # Safety
/// `ptr` came from `Box::into_raw` in receive and was not reclaimed before.
pub unsafe fn release<T: Held + ?Sized>(ptr: *mut T) -> Box<T> {
    crate::exchange::forget_held(ptr.cast::<()>());
    let st = unsafe { Box::from_raw(ptr) };
    if !st.settled() {
        sb_update(Event::Handled(true));
    }
    st
}

/// One unit of work on the intake. The PHP thread sees only this trait.
pub trait Work: Send + 'static {
    /// The client left while the unit was queued: receive() skips it.
    fn cancelled(&self) -> bool;
    /// Dispatcher mode: attaches the unit to the object receive() allocated from `DispatcherClasses::unit`.
    /// # Safety
    /// `obj` is a live object of that class on the PHP thread.
    unsafe fn attach(self: Box<Self>, obj: *mut zend_object) -> *mut dyn Held;
    /// The classic and worker modes.
    fn into_cgi(self: Box<Self>) -> crate::types::Context;
    /// A worker that cannot serve: the plugin's refusal, 503 or UNAVAILABLE.
    fn shed(self: Box<Self>);
}

/// A queued unit owns its permits until dequeue or queue destruction.
pub(crate) struct Entry {
    pub(crate) unit: Box<dyn Work>,
    _bytes: OwnedSemaphorePermit,
    _slot: OwnedSemaphorePermit,
}

/// The class entries of one plugin's dispatcher surface. Set once per worker.
#[derive(Clone, Copy)]
pub struct DispatcherClasses {
    pub dispatcher: unsafe fn() -> *mut zend_class_entry,
    pub info: unsafe fn() -> *mut zend_class_entry,
    pub unit: unsafe fn() -> *mut zend_class_entry,
    /// The receive() error while a unit is unfinalized.
    pub busy: &'static CStr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    Saturated,
    OverBudget,
    Stopped,
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Saturated => write!(f, "worker pool saturated for {INTAKE_WAIT:?}"),
            Self::OverBudget => write!(
                f,
                "request bodies use all {} MiB of the pool budget",
                BODY_BUDGET >> 20
            ),
            Self::Stopped => write!(f, "worker pool stopped"),
        }
    }
}

impl std::error::Error for Refused {}

pub fn now_unix_f64() -> f64 {
    std::time::UNIX_EPOCH
        .elapsed()
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// The queue from a plugin thread to the PHP thread. Clone per plugin task.
#[derive(Clone)]
pub struct Intake {
    tx: Sender<Entry>,
    pool: Arc<Pool>,
    budget: Arc<Semaphore>,
    slots: Arc<Semaphore>,
}

struct PendingGuard<'a> {
    pool: Option<&'a Pool>,
    found_full: bool,
}

impl<'a> PendingGuard<'a> {
    fn arm(pool: &'a Pool) -> Self {
        pool.pending.fetch_add(1, Ordering::Relaxed);
        Self {
            pool: Some(pool),
            found_full: false,
        }
    }
    fn disarm(mut self) {
        self.pool = None;
    }
}

impl Drop for PendingGuard<'_> {
    fn drop(&mut self) {
        if let Some(pool) = self.pool.take() {
            pool.pending.fetch_sub(1, Ordering::Relaxed);
            if self.found_full {
                pool.failed_on_full_queue.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

impl Intake {
    pub(crate) fn new(tx: Sender<Entry>, pool: Arc<Pool>) -> Self {
        Self {
            tx,
            pool,
            budget: Arc::new(Semaphore::new(BODY_BUDGET)),
            slots: Arc::new(Semaphore::new(INTAKE_SLOTS)),
        }
    }

    pub(crate) fn close(&self) {
        self.budget.close();
        self.slots.close();
    }

    /// Reserve body bytes before buffering. One body uses at most the full pool budget.
    pub fn reserve_body(&self, bytes: usize) -> Result<OwnedSemaphorePermit, Refused> {
        match Arc::clone(&self.budget).try_acquire_many_owned(bytes.min(BODY_BUDGET) as u32) {
            Ok(held) => Ok(held),
            Err(TryAcquireError::Closed) => Err(Refused::Stopped),
            Err(TryAcquireError::NoPermits) => {
                self.pool.shed.fetch_add(1, Ordering::Relaxed);
                Err(Refused::OverBudget)
            }
        }
    }

    /// Grow a streamed body's reservation without waiting on other incomplete bodies.
    pub fn grow_body(&self, held: &mut OwnedSemaphorePermit, bytes: usize) -> Result<(), Refused> {
        let extra = bytes.min(BODY_BUDGET).saturating_sub(held.num_permits());
        if extra > 0 {
            held.merge(self.reserve_body(extra)?);
        }
        Ok(())
    }

    /// Keep the body reservation until dequeue. Increment pending before send because the consumer can wake immediately.
    pub async fn submit(
        &self,
        unit: Box<dyn Work>,
        held: OwnedSemaphorePermit,
    ) -> Result<(), Refused> {
        let mut pending = PendingGuard::arm(&self.pool);
        let slot = match Arc::clone(&self.slots).try_acquire_owned() {
            Ok(slot) => slot,
            Err(TryAcquireError::Closed) => return Err(Refused::Stopped),
            Err(TryAcquireError::NoPermits) => {
                pending.found_full = true;
                match tokio::time::timeout(INTAKE_WAIT, Arc::clone(&self.slots).acquire_owned())
                    .await
                {
                    Ok(Ok(slot)) => slot,
                    Ok(Err(_)) => return Err(Refused::Stopped),
                    Err(_) => {
                        tracing::warn!(
                            target: "rapira",
                            "intake full for {INTAKE_WAIT:?} ({} pending); shedding the request",
                            self.pool.pending.load(Ordering::Relaxed)
                        );
                        self.pool.shed.fetch_add(1, Ordering::Relaxed);
                        return Err(Refused::Saturated);
                    }
                }
            }
        };
        let entry = Entry {
            unit,
            _bytes: held,
            _slot: slot,
        };
        if self.tx.send(entry).is_err() {
            return Err(Refused::Stopped);
        }
        pending.disarm();
        Ok(())
    }
}

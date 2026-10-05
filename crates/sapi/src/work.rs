use crossbeam_channel::{Sender, TrySendError};
use std::ffi::CStr;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use crate::scoreboard::{Event, sb_update};
use crate::{zend_class_entry, zend_object};
use rapira_scoreboard::Pool;

pub(crate) const INTAKE_WAIT: Duration = Duration::from_secs(30);

/// The cycle bookkeeping view of a unit that receive() handed out.
pub trait Held {
    /// The worker committed the outcome, or discarded the unit.
    fn finalized(&self) -> bool;
    /// The unit takes no outcome: the client left, or the plugin closed the unit. Work::isCancelled().
    fn client_closed(&self) -> bool;
    fn discard(&mut self);

    /// Work::isFinalized().
    fn is_finalized(&self) -> bool {
        self.finalized() || self.client_closed()
    }
}

/// Reclaims the Box that receive() handed out: clears the cycle slot if it still points here, and counts an unfinalized unit as handled.
/// # Safety
/// `ptr` came from `Box::into_raw` in receive and was not reclaimed before.
pub unsafe fn release<T: Held + ?Sized>(ptr: *mut T) -> Box<T> {
    crate::exchange::forget_held(ptr.cast::<()>());
    let st = unsafe { Box::from_raw(ptr) };
    if !st.finalized() {
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
    /// The classic and worker modes. None: this unit cannot run there.
    fn into_cgi(self: Box<Self>) -> Option<crate::types::Context>;
    /// A worker that cannot serve: the plugin's refusal, 503 or UNAVAILABLE.
    fn shed(self: Box<Self>);
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
    Stopped,
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Saturated => write!(f, "worker pool saturated for {INTAKE_WAIT:?}"),
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
pub struct Sink {
    tx: Sender<Box<dyn Work>>,
    pool: Arc<Pool>,
}

struct PendingGuard<'a> {
    pool: Option<&'a Pool>,
    deadline: Option<Instant>,
}

impl<'a> PendingGuard<'a> {
    fn arm(pool: &'a Pool) -> Self {
        pool.pending.fetch_add(1, Ordering::Relaxed);
        Self {
            pool: Some(pool),
            deadline: None,
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
            if self.deadline.is_some() {
                pool.failed_on_full_queue.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

impl Sink {
    pub(crate) fn new(tx: Sender<Box<dyn Work>>, pool: Arc<Pool>) -> Self {
        Self { tx, pool }
    }

    /// pending is incremented before the send: the consumer decrements as soon as it wakes, so the reverse order could wrap the counter below zero.
    pub async fn submit(&self, mut unit: Box<dyn Work>) -> Result<(), Refused> {
        let mut pending = PendingGuard::arm(&self.pool);
        loop {
            match self.tx.try_send(unit) {
                Ok(()) => {
                    pending.disarm();
                    return Ok(());
                }
                Err(TrySendError::Full(u)) => {
                    let deadline = *pending
                        .deadline
                        .get_or_insert_with(|| Instant::now() + INTAKE_WAIT);
                    if Instant::now() > deadline {
                        tracing::warn!(
                            target: "rapira",
                            "intake full for {INTAKE_WAIT:?} ({} pending); shedding the request",
                            self.pool.pending.load(Ordering::Relaxed)
                        );
                        self.pool.shed.fetch_add(1, Ordering::Relaxed);
                        return Err(Refused::Saturated);
                    }
                    unit = u;
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
                Err(TrySendError::Disconnected(_)) => return Err(Refused::Stopped),
            }
        }
    }
}

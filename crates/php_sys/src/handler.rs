use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use crossbeam_channel::{Sender, TrySendError};
use tokio::sync::{mpsc, oneshot};

use crate::{
    start::Intake,
    types::{Context, Frame, GrpcJob, GrpcOutcome, GrpcRequest, Request, Unit},
};

// A capacity of four accepts a buffered Head, Chunk, and End group plus one interim head without blocking the PHP thread.
const FRAME_CAP: usize = 4;

const INTAKE_WAIT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandleError {
    Saturated,
    Stopped,
}

impl std::fmt::Display for HandleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Saturated => write!(f, "worker pool saturated for {INTAKE_WAIT:?}"),
            Self::Stopped => write!(f, "worker pool stopped"),
        }
    }
}

impl std::error::Error for HandleError {}

#[derive(Clone)]
pub struct RapiraHandle {
    intake: Sender<Unit>,
    pending: Arc<AtomicUsize>,
    dispatcher: bool,
}

impl RapiraHandle {
    pub(crate) fn new(intake: &Intake, dispatcher: bool) -> Self {
        Self {
            intake: intake.tx.clone(),
            pending: intake.pending.clone(),
            dispatcher,
        }
    }
}

fn now_unix_f64() -> f64 {
    std::time::UNIX_EPOCH
        .elapsed()
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

struct PendingGuard<'a>(Option<&'a AtomicUsize>);

impl<'a> PendingGuard<'a> {
    fn arm(pending: &'a AtomicUsize) -> Self {
        pending.fetch_add(1, Ordering::Relaxed);
        Self(Some(pending))
    }
    fn disarm(mut self) {
        self.0 = None;
    }
}

impl Drop for PendingGuard<'_> {
    fn drop(&mut self) {
        if let Some(pending) = self.0.take() {
            pending.fetch_sub(1, Ordering::Relaxed);
        }
    }
}

impl RapiraHandle {
    pub fn dispatcher(&self) -> bool {
        self.dispatcher
    }

    pub async fn handle(&self, mut req: Request) -> Result<mpsc::Receiver<Frame>, HandleError> {
        req.received_at.get_or_insert_with(now_unix_f64);
        let (tx, rx) = mpsc::channel::<Frame>(FRAME_CAP);
        let job = Box::new(Context::new(req, tx, !self.dispatcher));
        self.enqueue(Unit::Http(job)).await?;
        Ok(rx)
    }

    /// Queues one unary gRPC call. A dropped sender means that PHP lost the call. Dropping the receiver closes the call for PHP.
    pub async fn call(
        &self,
        req: GrpcRequest,
    ) -> Result<oneshot::Receiver<GrpcOutcome>, HandleError> {
        let (reply, rx) = oneshot::channel();
        let job = Box::new(GrpcJob {
            req,
            received_at: now_unix_f64(),
            reply,
        });
        self.enqueue(Unit::Grpc(job)).await?;
        Ok(rx)
    }

    // Increment pending before the send. The consumer decrements it as soon as the consumer resumes, so the opposite order could wrap the counter below zero.
    async fn enqueue(&self, mut unit: Unit) -> Result<(), HandleError> {
        let pending = PendingGuard::arm(&self.pending);
        let deadline = Instant::now() + INTAKE_WAIT;
        loop {
            match self.intake.try_send(unit) {
                Ok(()) => {
                    pending.disarm();
                    return Ok(());
                }
                Err(TrySendError::Full(u)) => {
                    if Instant::now() > deadline {
                        tracing::warn!(
                            target: "rapira",
                            "intake full for {INTAKE_WAIT:?} ({} pending); shedding the request",
                            self.pending.load(Ordering::Relaxed)
                        );
                        return Err(HandleError::Saturated);
                    }
                    unit = u;
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
                Err(TrySendError::Disconnected(_)) => return Err(HandleError::Stopped),
            }
        }
    }
}

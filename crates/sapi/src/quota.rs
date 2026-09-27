use std::cell::RefCell;
use std::sync::Arc;

use crate::scoreboard::{Event, sb_update};

#[derive(Clone)]
pub struct PoolHooks {
    pub max_requests: u64,
    pub on_boot_failure: Arc<dyn Fn() + Send + Sync>,
}

#[derive(Default)]
struct QuotaState {
    served: u64,
    max: u64,
    draining: bool,
    unhealthy: bool,
}

thread_local! {
    static Q: RefCell<QuotaState> = RefCell::new(QuotaState::default());
}

pub(crate) fn install(max: u64) {
    Q.with_borrow_mut(|q| {
        *q = QuotaState {
            max,
            ..QuotaState::default()
        }
    });
}

pub(crate) fn tick() {
    crate::start::note_handled();
    Q.with_borrow_mut(|q| {
        if q.max == 0 || q.draining {
            return;
        }
        q.served += 1;
        if q.served == q.max {
            tracing::info!(target: "rapira", "worker served {} requests; recycling", q.served);
            q.draining = true;
            sb_update(Event::Draining);
        }
    });
}

pub(crate) fn fire_unhealthy() {
    Q.with_borrow_mut(|q| {
        q.unhealthy = true;
        q.draining = true;
    });
    sb_update(Event::Draining);
}

pub(crate) fn is_draining() -> bool {
    Q.with_borrow(|q| q.draining)
}
pub(crate) fn is_unhealthy() -> bool {
    Q.with_borrow(|q| q.unhealthy)
}

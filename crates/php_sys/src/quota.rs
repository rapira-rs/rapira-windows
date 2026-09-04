use std::cell::RefCell;
use std::sync::Arc;

use tracing::info;

use crate::scoreboard::{Event, sb_update};

#[derive(Clone)]
pub struct PoolHooks {
    /// Zero permits an unlimited number of requests per interpreter.
    pub max_requests: u64,
    pub on_boot_failure: Arc<dyn Fn() + Send + Sync>,
}

impl Default for PoolHooks {
    fn default() -> Self {
        Self {
            max_requests: 0,
            on_boot_failure: Arc::new(|| {}),
        }
    }
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

/// Resets the quota and drain state before each interpreter generation.
pub(crate) fn install(max_requests: u64) {
    Q.with_borrow_mut(|q| {
        *q = QuotaState {
            max: max_requests,
            ..QuotaState::default()
        };
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
            info!(target: "rapira", "worker served {} requests; recycling", q.served);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quota_drains_at_the_limit_and_resets_each_generation() {
        install(3);
        tick();
        tick();
        assert!(!is_draining());
        tick();
        assert!(is_draining());
        assert!(!is_unhealthy());

        install(3);
        tick();
        tick();
        assert!(!is_draining());
        tick();
        assert!(is_draining());
    }

    #[test]
    fn unhealthy_drains_an_unlimited_generation() {
        install(0);
        for _ in 0..10 {
            tick();
        }
        assert!(!is_draining());
        fire_unhealthy();
        assert!(is_draining());
        assert!(is_unhealthy());

        install(0);
        assert!(!is_draining());
        assert!(!is_unhealthy());
    }

    #[test]
    fn quota_state_is_local_to_the_worker_thread() {
        install(0);
        std::thread::spawn(|| {
            install(1);
            tick();
            fire_unhealthy();
            assert!(is_draining());
            assert!(is_unhealthy());
        })
        .join()
        .unwrap();

        assert!(!is_draining());
        assert!(!is_unhealthy());
    }
}

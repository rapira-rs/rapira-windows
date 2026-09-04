use std::cell::Cell;
use std::sync::atomic::Ordering::{Relaxed, Release};

use rapira_scoreboard::{SLOT_ACTIVE, SLOT_DRAINING, SLOT_IDLE, SharedSlot, now_millis};

pub use rapira_scoreboard::SlotSnapshot;

thread_local! {
    pub static SB: Cell<Option<&'static SharedSlot>> = const { Cell::new(None) };
    static DRAINING: Cell<bool> = const { Cell::new(false) };
}

pub enum Event {
    Handled(bool),
    Shed,
    Recycled,
    Restart,
    Unhealthy,
    Healthy,
    Idle,
    Active,
    Draining,
}

pub fn sb_set(slot: &'static SharedSlot) {
    SB.set(Some(slot));
    DRAINING.set(false);
}

/// Each Release store publishes the preceding Relaxed write to an Acquire load by a reader on another thread.
pub fn sb_update(event: Event) {
    let Some(s) = SB.get() else { return };
    match event {
        Event::Handled(errored) => {
            if errored {
                s.errors.fetch_add(1, Relaxed);
            }
            s.handled.fetch_add(1, Release);
            crate::quota::tick();
        }
        Event::Shed => {
            s.errors.fetch_add(1, Relaxed);
            s.handled.fetch_add(1, Release);
        }
        Event::Recycled => {
            s.recycles.fetch_add(1, Relaxed);
        }
        Event::Restart => {
            s.restarts.fetch_add(1, Relaxed);
        }
        Event::Unhealthy => {
            s.unhealthy.store(1, Relaxed);
            crate::quota::fire_unhealthy();
        }
        Event::Healthy => s.unhealthy.store(0, Relaxed),
        Event::Idle => {
            let state = if DRAINING.get() {
                SLOT_DRAINING
            } else {
                SLOT_IDLE
            };
            s.last_activity_ms.store(now_millis(), Relaxed);
            s.state.store(state, Release);
        }
        Event::Active => {
            s.last_activity_ms.store(now_millis(), Relaxed);
            s.state.store(SLOT_ACTIVE, Release);
        }
        Event::Draining => DRAINING.set(true),
    }
}

#[derive(Debug, Default, Clone)]
pub struct ScoreboardSnapshot {
    pub handled: u64,
    pub errors: u64,
    pub recycles: u64,
    pub restarts: u64,
    pub unhealthy: usize,
    pub workers: Vec<SlotSnapshot>,
}

pub(crate) fn snapshot(board: &rapira_scoreboard::Scoreboard) -> ScoreboardSnapshot {
    let workers = board.snapshot_slots();
    ScoreboardSnapshot {
        handled: workers.iter().map(|w| w.handled).sum(),
        errors: workers.iter().map(|w| w.errors).sum(),
        recycles: workers.iter().map(|w| w.recycles).sum(),
        restarts: workers.iter().map(|w| w.restarts).sum(),
        unhealthy: workers.iter().filter(|w| w.unhealthy).count(),
        workers,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rebuilt_interpreter_returns_to_idle_and_keeps_totals() {
        let board = rapira_scoreboard::Scoreboard::create(1).unwrap();
        let slot = board.slot(0).unwrap();
        slot.bind(0);
        sb_set(slot);
        sb_update(Event::Shed);
        sb_update(Event::Recycled);
        sb_update(Event::Draining);
        sb_update(Event::Idle);
        assert_eq!(board.snapshot_slots()[0].state, SLOT_DRAINING);

        sb_set(slot);
        sb_update(Event::Idle);
        let state = snapshot(&board);
        assert_eq!(state.workers[0].state, SLOT_IDLE);
        assert_eq!(state.handled, 1);
        assert_eq!(state.recycles, 1);
    }
}

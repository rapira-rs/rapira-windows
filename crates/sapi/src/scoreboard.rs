use std::cell::Cell;
use std::sync::atomic::Ordering::{Relaxed, Release};

use rapira_scoreboard::{SLOT_ACTIVE, SLOT_DRAINING, SLOT_IDLE, SharedSlot};

thread_local! {
    pub static SB: Cell<Option<&'static SharedSlot>> = const { Cell::new(None) };
    static DRAINING: Cell<bool> = const { Cell::new(false) };
}

pub enum Event {
    Handled(bool),
    Shed,
    Recycled,
    Unhealthy,
    Idle,
    Active,
    Draining,
}

pub fn sb_set(slot: &'static SharedSlot) {
    DRAINING.set(false);
    SB.set(Some(slot));
}

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
        Event::Unhealthy => crate::quota::fire_unhealthy(),
        Event::Idle => {
            let state = if DRAINING.get() {
                SLOT_DRAINING
            } else {
                SLOT_IDLE
            };
            s.state.store(state, Release);
        }
        Event::Active => {
            s.state.store(SLOT_ACTIVE, Release);
        }
        Event::Draining => DRAINING.set(true),
    }
}

use std::cell::Cell;
use std::sync::atomic::Ordering::{Relaxed, Release};

use rapira_scoreboard::{SLOT_ACTIVE, SLOT_DRAINING, SLOT_IDLE, SLOT_STARTING, SharedSlot};

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
    BootFailed,
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
        Event::Idle => s.state.store(draining_or(SLOT_IDLE), Release),
        Event::Active => {
            s.state.store(SLOT_ACTIVE, Release);
        }
        Event::Draining => {
            DRAINING.set(true);
            s.state.store(SLOT_DRAINING, Release);
        }
        Event::BootFailed => s.state.store(draining_or(SLOT_STARTING), Release),
    }
}

fn draining_or(state: u32) -> u32 {
    if DRAINING.get() { SLOT_DRAINING } else { state }
}

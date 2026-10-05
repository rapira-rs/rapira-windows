use std::sync::atomic::{
    AtomicU32, AtomicU64,
    Ordering::{Relaxed, Release},
};

pub const SB_MAX_SLOTS: usize = 4096;

pub const SLOT_FREE: u32 = 0;
pub const SLOT_STARTING: u32 = 1;
pub const SLOT_IDLE: u32 = 2;
pub const SLOT_ACTIVE: u32 = 3;
pub const SLOT_DRAINING: u32 = 4;

/// One slot belongs to one interpreter thread. Its counters survive interpreter recycling.
#[derive(Default)]
#[repr(C, align(64))]
pub struct SharedSlot {
    pub state: AtomicU32,
    pub pid: AtomicU32,
    pub handled: AtomicU64,
    pub errors: AtomicU64,
    pub recycles: AtomicU64,
}

const _: () = assert!(size_of::<SharedSlot>() == 64 && align_of::<SharedSlot>() == 64);

/// Allocates the slots before the interpreter threads start.
pub fn create(nslots: usize) -> anyhow::Result<&'static [SharedSlot]> {
    anyhow::ensure!(
        (1..=SB_MAX_SLOTS).contains(&nslots),
        "scoreboard slots out of range: {nslots}"
    );
    Ok(Box::leak(
        (0..nslots)
            .map(|_| SharedSlot::default())
            .collect::<Box<[_]>>(),
    ))
}

impl SharedSlot {
    /// Claims the slot once for its interpreter thread, before requests arrive.
    pub fn bind(&self, pid: u32) {
        self.pid.store(pid, Relaxed);
        self.state.store(SLOT_STARTING, Release);
    }

    pub fn serving(&self) -> bool {
        matches!(self.state.load(Relaxed), SLOT_IDLE | SLOT_ACTIVE)
    }
}

/// All interpreter threads in a pool share one queue.
pub struct Pool {
    pub name: &'static str,
    pub slots: &'static [SharedSlot],
    pub pending: AtomicU64,
    pub failed_on_full_queue: AtomicU64,
    pub shed: AtomicU64,
}

impl Pool {
    pub fn new(name: &'static str, slots: &'static [SharedSlot]) -> Self {
        Self {
            name,
            slots,
            pending: AtomicU64::new(0),
            failed_on_full_queue: AtomicU64::new(0),
            shed: AtomicU64::new(0),
        }
    }
}

use std::ops::Range;
use std::sync::OnceLock;
use std::sync::atomic::{
    AtomicU32, AtomicU64,
    Ordering::{Relaxed, Release},
};
use std::time::Instant;

pub const SB_MAX_SLOTS: usize = 4096;

pub const SLOT_FREE: u32 = 0;
pub const SLOT_STARTING: u32 = 1;
pub const SLOT_IDLE: u32 = 2;
pub const SLOT_ACTIVE: u32 = 3;
pub const SLOT_DRAINING: u32 = 4; // worker-initiated exit pending

/// The host reserves the slot before its interpreter starts. Only that interpreter updates it.
#[repr(C, align(64))]
pub struct SharedSlot {
    pub state: AtomicU32,
    pub pid: AtomicU32,
    pub handled: AtomicU64,
    pub errors: AtomicU64,
    pub recycles: AtomicU64,
    pub last_activity_ms: AtomicU64,
}

const _: () = assert!(size_of::<SharedSlot>() == 64 && align_of::<SharedSlot>() == 64);

/// The slot allocation remains valid for the lifetime of all interpreter threads.
#[derive(Clone, Copy)]
pub struct Scoreboard {
    slots: &'static [SharedSlot],
}

#[derive(Debug, Default, Clone)]
pub struct SlotSnapshot {
    pub id: usize,
    pub pid: u32,
    pub state: u32,
    pub handled: u64,
    pub errors: u64,
    pub recycles: u64,
}

/// Milliseconds from one process-wide monotonic clock.
pub fn now_millis() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

impl Scoreboard {
    /// Allocates one slot for each configured interpreter before the pools start.
    pub fn create(nslots: usize) -> anyhow::Result<Scoreboard> {
        anyhow::ensure!(
            (1..=SB_MAX_SLOTS).contains(&nslots),
            "scoreboard slots out of range: {nslots}"
        );
        let slots = (0..nslots)
            .map(|_| SharedSlot {
                state: AtomicU32::new(SLOT_FREE),
                pid: AtomicU32::new(0),
                handled: AtomicU64::new(0),
                errors: AtomicU64::new(0),
                recycles: AtomicU64::new(0),
                last_activity_ms: AtomicU64::new(0),
            })
            .collect::<Box<[_]>>();
        Ok(Scoreboard {
            slots: Box::leak(slots),
        })
    }

    /// View over `range` of this board: indices inside the view are local to it, the memory is shared.
    pub fn slice(&self, range: Range<usize>) -> Scoreboard {
        Scoreboard {
            slots: &self.slots[range],
        }
    }

    pub fn nslots(&self) -> usize {
        self.slots.len()
    }

    pub fn slot(&self, i: usize) -> &'static SharedSlot {
        &self.slots[i]
    }

    /// Reserves the slot before the host starts its interpreter thread.
    pub fn set_starting(&self, i: usize) {
        let s = self.slot(i);
        s.last_activity_ms.store(now_millis(), Relaxed);
        s.state.store(SLOT_STARTING, Release);
    }

    pub fn snapshot_slots(&self) -> Vec<SlotSnapshot> {
        self.slots
            .iter()
            .enumerate()
            .filter(|(_, s)| s.state.load(Relaxed) != SLOT_FREE || s.pid.load(Relaxed) != 0)
            .map(|(id, s)| SlotSnapshot {
                id,
                pid: s.pid.load(Relaxed),
                state: s.state.load(Relaxed),
                handled: s.handled.load(Relaxed),
                errors: s.errors.load(Relaxed),
                recycles: s.recycles.load(Relaxed),
            })
            .collect()
    }
}

impl SharedSlot {
    /// Claims the slot once for its interpreter thread, before requests arrive.
    pub fn bind(&'static self, pid: u32) {
        self.handled.store(0, Relaxed);
        self.errors.store(0, Relaxed);
        self.recycles.store(0, Relaxed);
        self.pid.store(pid, Relaxed);
        self.last_activity_ms.store(now_millis(), Relaxed);
        self.state.store(SLOT_IDLE, Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_bind_snapshot_roundtrip() {
        let sb = Scoreboard::create(3).unwrap();
        assert_eq!(sb.nslots(), 3);
        assert_eq!(sb.slot(0).state.load(Relaxed), SLOT_FREE);

        sb.set_starting(0);
        assert_eq!(sb.slot(0).state.load(Relaxed), SLOT_STARTING);
        assert_eq!(sb.slot(1).state.load(Relaxed), SLOT_FREE);

        let slot = sb.slot(0);
        slot.bind(4242);
        slot.handled.fetch_add(2, Relaxed);
        slot.errors.fetch_add(1, Relaxed);

        let snap = sb.snapshot_slots();
        assert_eq!(snap.len(), 1);
        assert_eq!((snap[0].pid, snap[0].handled, snap[0].errors), (4242, 2, 1));
        assert_eq!(snap[0].state, SLOT_IDLE);
    }

    #[test]
    fn slice_shares_memory_with_local_indices() {
        let board = Scoreboard::create(6).unwrap();
        let view = board.slice(2..4);
        assert_eq!(view.nslots(), 2);
        assert!(std::ptr::eq(view.slot(1), board.slot(3)));

        view.set_starting(0);
        assert_eq!(board.slot(2).state.load(Relaxed), SLOT_STARTING);
        assert_eq!(board.slot(1).state.load(Relaxed), SLOT_FREE);

        assert_eq!(view.snapshot_slots()[0].id, 0);
    }
}

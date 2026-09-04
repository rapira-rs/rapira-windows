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
pub const SLOT_DRAINING: u32 = 4; // The worker requested exit.

/// Each worker thread writes its slot. The boot thread sets STARTING before spawn and FREE after join.
#[repr(C, align(64))]
pub struct SharedSlot {
    pub state: AtomicU32,
    /// Worker thread index on Windows.
    pub pid: AtomicU32,
    pub handled: AtomicU64,
    pub errors: AtomicU64,
    pub recycles: AtomicU64,
    pub restarts: AtomicU64,
    pub unhealthy: AtomicU32,
    _pad: [u8; 4],
    pub last_activity_ms: AtomicU64,
    _tail: [u8; 8],
}

const _: () = assert!(size_of::<SharedSlot>() == 64 && align_of::<SharedSlot>() == 64);

/// Copy of the slot table. [`Box::leak`](https://doc.rust-lang.org/std/boxed/struct.Box.html#method.leak) keeps the allocation valid for the process lifetime.
#[derive(Clone, Copy)]
pub struct Scoreboard {
    slots: &'static [SharedSlot],
}

#[derive(Debug, Default, Clone)]
pub struct SlotSnapshot {
    pub id: usize,
    /// Worker thread index on Windows.
    pub pid: u32,
    pub state: u32,
    pub handled: u64,
    pub errors: u64,
    pub recycles: u64,
    pub restarts: u64,
    pub unhealthy: bool,
    pub last_activity_ms: u64,
}

/// Milliseconds from a process-wide monotonic [`Instant`](https://doc.rust-lang.org/std/time/struct.Instant.html).
pub fn now_millis() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

impl Scoreboard {
    /// Creates one slot for each worker thread before the workers start.
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
                restarts: AtomicU64::new(0),
                unhealthy: AtomicU32::new(0),
                _pad: [0; 4],
                last_activity_ms: AtomicU64::new(0),
                _tail: [0; 8],
            })
            .collect::<Box<[_]>>();
        Ok(Scoreboard {
            slots: Box::leak(slots),
        })
    }

    pub fn nslots(&self) -> usize {
        self.slots.len()
    }

    pub fn slot(&self, i: usize) -> Option<&'static SharedSlot> {
        self.slots.get(i)
    }

    pub fn slots(&self) -> &'static [SharedSlot] {
        self.slots
    }

    /// The boot thread reserves the slot before its worker starts.
    pub fn set_starting(&self, i: usize) {
        if let Some(s) = self.slots.get(i) {
            s.last_activity_ms.store(now_millis(), Relaxed);
            s.state.store(SLOT_STARTING, Release);
        }
    }

    /// The boot thread clears the slot after its worker joins.
    pub fn clear(&self, i: usize) {
        if let Some(s) = self.slots.get(i) {
            s.pid.store(0, Relaxed);
            s.state.store(SLOT_FREE, Relaxed);
        }
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
                restarts: s.restarts.load(Relaxed),
                unhealthy: s.unhealthy.load(Relaxed) != 0,
                last_activity_ms: s.last_activity_ms.load(Relaxed),
            })
            .collect()
    }
}

impl SharedSlot {
    /// Call this once for each worker thread before its first interpreter. The counts include all interpreter generations.
    pub fn bind(&'static self, pid: u32) {
        self.handled.store(0, Relaxed);
        self.errors.store(0, Relaxed);
        self.recycles.store(0, Relaxed);
        self.restarts.store(0, Relaxed);
        self.unhealthy.store(0, Relaxed);
        self.pid.store(pid, Relaxed);
        self.last_activity_ms.store(now_millis(), Relaxed);
        self.state.store(SLOT_IDLE, Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering::Acquire;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn worker_threads_have_independent_slots() {
        let sb = Scoreboard::create(3).unwrap();
        let workers: Vec<_> = [3, 5, 7]
            .into_iter()
            .enumerate()
            .map(|(index, handled)| {
                thread::spawn(move || {
                    let slot = sb.slot(index).unwrap();
                    slot.bind(index as u32);
                    slot.handled.fetch_add(handled, Relaxed);
                    slot.recycles.fetch_add(index as u64 + 1, Relaxed);
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }

        let snapshots = sb.snapshot_slots();
        let actual: Vec<_> = snapshots
            .iter()
            .map(|slot| (slot.id, slot.pid, slot.handled, slot.recycles))
            .collect();
        assert_eq!(actual, vec![(0, 0, 3, 1), (1, 1, 5, 2), (2, 2, 7, 3)]);
    }

    #[test]
    fn counters_survive_interpreter_generations() {
        let sb = Scoreboard::create(1).unwrap();
        let slot = sb.slot(0).unwrap();
        slot.bind(5);

        for (handled, total, recycles) in [(2, 2, 1), (3, 5, 2)] {
            slot.state.store(SLOT_IDLE, Relaxed);
            slot.handled.fetch_add(handled, Relaxed);
            slot.state.store(SLOT_DRAINING, Release);
            slot.recycles.fetch_add(1, Relaxed);

            let snapshot = &sb.snapshot_slots()[0];
            assert_eq!(snapshot.pid, 5);
            assert_eq!(snapshot.handled, total);
            assert_eq!(snapshot.recycles, recycles);
        }
    }

    #[test]
    fn activity_timestamps_use_monotonic_milliseconds() {
        let sb = Scoreboard::create(1).unwrap();
        let before = now_millis();
        sb.set_starting(0);
        let starting = sb.slot(0).unwrap().last_activity_ms.load(Relaxed);
        let bound = thread::spawn(move || {
            let slot = sb.slot(0).unwrap();
            slot.bind(0);
            slot.last_activity_ms.load(Relaxed)
        })
        .join()
        .unwrap();
        let after = now_millis();

        assert!(before <= starting);
        assert!(starting <= bound);
        assert!(bound <= after);
        thread::sleep(Duration::from_millis(2));
        assert!(now_millis() >= after + 2);
    }

    #[test]
    fn published_slot_is_visible_to_another_thread() {
        let sb = Scoreboard::create(1).unwrap();
        let slot = sb.slot(0).unwrap();
        slot.bind(7);
        let reader = thread::spawn(move || {
            while slot.state.load(Acquire) != SLOT_DRAINING {
                thread::yield_now();
            }
            let snapshot = &sb.snapshot_slots()[0];
            assert_eq!(snapshot.pid, 7);
            assert_eq!(snapshot.state, SLOT_DRAINING);
            assert_eq!(snapshot.handled, 9);
            assert_eq!(snapshot.recycles, 2);
        });

        slot.handled.store(9, Relaxed);
        slot.recycles.store(2, Relaxed);
        slot.state.store(SLOT_DRAINING, Release);
        reader.join().unwrap();
    }

    #[test]
    fn create_bind_snapshot_roundtrip() {
        let sb = Scoreboard::create(3).unwrap();
        assert_eq!(sb.nslots(), 3);
        assert_eq!(sb.slot(0).unwrap().state.load(Relaxed), SLOT_FREE);

        sb.set_starting(0);
        assert_eq!(sb.slot(0).unwrap().state.load(Relaxed), SLOT_STARTING);
        assert_eq!(sb.slot(1).unwrap().state.load(Relaxed), SLOT_FREE);

        let slot = sb.slot(0).unwrap();
        slot.bind(4242);
        slot.handled.fetch_add(2, Relaxed);
        slot.errors.fetch_add(1, Relaxed);

        let snap = sb.snapshot_slots();
        assert_eq!(snap.len(), 1);
        assert_eq!((snap[0].pid, snap[0].handled, snap[0].errors), (4242, 2, 1));
        assert_eq!(snap[0].state, SLOT_IDLE);

        sb.clear(0);
        assert_eq!(sb.slot(0).unwrap().state.load(Relaxed), SLOT_FREE);
        assert!(sb.snapshot_slots().is_empty());
    }

    #[test]
    fn slots_out_of_range_rejected() {
        assert!(Scoreboard::create(0).is_err());
        assert!(Scoreboard::create(SB_MAX_SLOTS + 1).is_err());
    }
}

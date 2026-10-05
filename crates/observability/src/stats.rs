use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;

use rapira_scoreboard::{Pool, SLOT_ACTIVE, SLOT_DRAINING, SLOT_IDLE, SLOT_STARTING};

pub(crate) const STATES: [(u32, &str); 4] = [
    (SLOT_STARTING, "starting"),
    (SLOT_IDLE, "idle"),
    (SLOT_ACTIVE, "active"),
    (SLOT_DRAINING, "draining"),
];

#[derive(Debug, Default, PartialEq)]
pub(crate) struct PoolStats {
    pub name: &'static str,
    pub configured: usize,
    pub states: [u64; 4],
    pub requests: u64,
    pub failed: u64,
    pub failed_on_full_queue: u64,
    pub queued: u64,
    pub script_restarts: u64,
}

pub(crate) fn board_stats(pools: &[Arc<Pool>]) -> Vec<PoolStats> {
    pools
        .iter()
        .map(|pool| {
            let shed = pool.shed.load(Relaxed);
            let mut stats = PoolStats {
                name: pool.name,
                configured: pool.slots.len(),
                requests: shed,
                failed: shed,
                failed_on_full_queue: pool.failed_on_full_queue.load(Relaxed),
                queued: pool.pending.load(Relaxed),
                ..Default::default()
            };
            for slot in pool.slots {
                stats.requests += slot.handled.load(Relaxed);
                stats.failed += slot.errors.load(Relaxed);
                stats.script_restarts += slot.recycles.load(Relaxed);
                let state = slot.state.load(Relaxed);
                if slot.pid.load(Relaxed) != 0
                    && let Some(i) = STATES.iter().position(|&(value, _)| value == state)
                {
                    stats.states[i] += 1;
                }
            }
            stats
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rapira_scoreboard::SLOT_FREE;
    use std::sync::atomic::Ordering::Relaxed;

    #[test]
    fn counts_retained_work_and_each_shared_queue_once() {
        struct Case {
            name: &'static str,
            states: [u32; 5],
            expected_states: [[u64; 4]; 2],
        }
        let cases = [
            Case {
                name: "two pools with different live states",
                states: [
                    SLOT_IDLE,
                    SLOT_ACTIVE,
                    SLOT_STARTING,
                    SLOT_DRAINING,
                    SLOT_FREE,
                ],
                expected_states: [[1, 1, 1, 0], [0, 0, 0, 1]],
            },
            Case {
                name: "stopped slots retain totals",
                states: [SLOT_FREE; 5],
                expected_states: [[0; 4]; 2],
            },
        ];
        for case in cases {
            let slots = rapira_scoreboard::create(5).unwrap();
            for (slot, state) in slots.iter().zip(case.states) {
                slot.pid
                    .store(if state == SLOT_FREE { 0 } else { 301 }, Relaxed);
                slot.state.store(state, Relaxed);
                slot.handled.store(10, Relaxed);
                slot.errors.store(2, Relaxed);
                slot.recycles.store(1, Relaxed);
            }
            let http = Arc::new(Pool::new("http", &slots[..3]));
            http.pending.store(7, Relaxed);
            http.failed_on_full_queue.store(5, Relaxed);
            http.shed.store(3, Relaxed);
            let grpc = Arc::new(Pool::new("grpc", &slots[3..]));
            grpc.pending.store(4, Relaxed);
            grpc.failed_on_full_queue.store(2, Relaxed);
            grpc.shed.store(1, Relaxed);
            assert_eq!(
                board_stats(&[http, grpc]),
                vec![
                    PoolStats {
                        name: "http",
                        configured: 3,
                        states: case.expected_states[0],
                        requests: 33,
                        failed: 9,
                        failed_on_full_queue: 5,
                        queued: 7,
                        script_restarts: 3
                    },
                    PoolStats {
                        name: "grpc",
                        configured: 2,
                        states: case.expected_states[1],
                        requests: 21,
                        failed: 5,
                        failed_on_full_queue: 2,
                        queued: 4,
                        script_restarts: 2
                    },
                ],
                "{}",
                case.name
            );
        }
    }
}

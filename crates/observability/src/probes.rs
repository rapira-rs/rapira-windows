use std::sync::Arc;

use rapira_scoreboard::{Pool, SharedSlot};

pub(crate) fn unready(pools: &[Arc<Pool>]) -> Vec<&'static str> {
    pools
        .iter()
        .filter(|pool| !pool.slots.iter().any(SharedSlot::serving))
        .map(|pool| pool.name)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rapira_scoreboard::{SLOT_ACTIVE, SLOT_DRAINING, SLOT_FREE, SLOT_IDLE, SLOT_STARTING};
    use std::sync::atomic::Ordering::Relaxed;

    #[test]
    fn every_pool_needs_an_application_pull() {
        struct Case {
            name: &'static str,
            states: [u32; 4],
            expected: &'static [&'static str],
        }
        let cases = [
            Case {
                name: "all starting",
                states: [SLOT_STARTING; 4],
                expected: &["http", "grpc"],
            },
            Case {
                name: "idle http only",
                states: [SLOT_IDLE, SLOT_STARTING, SLOT_STARTING, SLOT_FREE],
                expected: &["grpc"],
            },
            Case {
                name: "active grpc only",
                states: [SLOT_FREE, SLOT_DRAINING, SLOT_ACTIVE, SLOT_STARTING],
                expected: &["http"],
            },
            Case {
                name: "one ready thread per pool",
                states: [SLOT_IDLE, SLOT_STARTING, SLOT_ACTIVE, SLOT_STARTING],
                expected: &[],
            },
            Case {
                name: "draining threads",
                states: [SLOT_DRAINING; 4],
                expected: &["http", "grpc"],
            },
        ];
        for case in cases {
            let slots = rapira_scoreboard::create(4).unwrap();
            for (slot, state) in slots.iter().zip(case.states) {
                slot.state.store(state, Relaxed);
            }
            let pools = [
                Arc::new(Pool::new("http", &slots[..2])),
                Arc::new(Pool::new("grpc", &slots[2..])),
            ];
            assert_eq!(unready(&pools), case.expected, "{}", case.name);
        }
    }
}

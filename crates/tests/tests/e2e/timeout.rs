use std::time::{Duration, Instant};

use super::harness::{BOOT, http_get, spawn_with_phprc_and_config, wait_log_contains};

#[test]
fn busy_loop_exceeds_max_execution_time() {
    let server = spawn_with_phprc_and_config(
        "timeout/budget-worker.php",
        1,
        "max_execution_time=1\ndisplay_errors=0\nlog_errors=1\n",
        "mode = \"worker\"\n",
    );
    assert!(wait_log_contains(&server, "worker thread 0 ready", BOOT));
    assert_eq!(
        http_get(server.addr, "/", BOOT).unwrap(),
        (200, b"ok".to_vec())
    );

    std::thread::sleep(Duration::from_secs(2));
    let started = Instant::now();
    let _ = http_get(server.addr, "/?spin=1", Duration::from_secs(10));
    assert!(
        wait_log_contains(&server, "Maximum execution time", Duration::from_secs(1)),
        "the PHP opcode loop must reach its one-second execution limit"
    );
    assert!(started.elapsed() < Duration::from_secs(10));
    assert_eq!(
        http_get(server.addr, "/", BOOT).unwrap(),
        (200, b"ok".to_vec())
    );
}

#[test]
fn recycled_interpreter_has_a_fresh_timer_budget() {
    let server = spawn_with_phprc_and_config(
        "timeout/recycle-worker.php",
        1,
        "max_execution_time=5\ndisplay_errors=0\nlog_errors=1\n",
        "mode = \"worker\"\nmax_requests = 1\n",
    );
    assert!(wait_log_contains(&server, "worker thread 0 ready", BOOT));
    // Core quota jitter makes max_requests=1 a two-request generation.
    assert_eq!(
        http_get(server.addr, "/", BOOT).unwrap(),
        (200, b"fresh".to_vec())
    );
    assert_eq!(
        http_get(server.addr, "/?arm=1", BOOT).unwrap(),
        (200, b"armed".to_vec())
    );
    assert!(wait_log_contains(
        &server,
        "worker thread 0 recycling",
        BOOT
    ));
    assert_eq!(
        http_get(server.addr, "/?work=1", Duration::from_secs(10)).unwrap(),
        (200, b"fresh".to_vec())
    );
    assert!(
        !wait_log_contains(&server, "Maximum execution time", Duration::ZERO),
        "the new interpreter must use its five-second budget"
    );
}

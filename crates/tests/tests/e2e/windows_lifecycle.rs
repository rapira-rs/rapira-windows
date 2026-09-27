use std::time::{Duration, Instant};

use rapira_sapi::Mode;
use tests::fixture;

use super::harness::{
    Conn, Spawn, assert_exit_code, diagnostics, http_get, send_ctrl_break, wait_log_contains,
};

const T: Duration = Duration::from_secs(10);
const DRAINING: &str = "shutdown requested; draining plugins";

#[test]
fn servers_have_separate_opcode_caches() {
    let target = fixture("opcache_isolation/target.php");
    let spawn = || {
        Spawn::http(Mode::Worker, fixture("opcache_isolation/worker.php"))
            .php_ini("opcache.file_update_protection=0")
            .env("RAPIRA_CACHE_FILE", target.to_str().unwrap())
            .spawn()
    };
    let first = spawn();
    let response = http_get(first.addr, "/compile", T).unwrap();
    if response.1 == b"skip" {
        tests::assert_skip_allowed("opcache");
        return;
    }
    assert_eq!(response, (200, b"cached".to_vec()));
    let second = spawn();
    assert_eq!(
        http_get(second.addr, "/", T).unwrap(),
        (200, b"missing".to_vec())
    );
    assert_eq!(
        http_get(first.addr, "/", T).unwrap(),
        (200, b"cached".to_vec())
    );
}

fn server(budget: u64) -> super::harness::Server {
    Spawn::http(Mode::Worker, fixture("windows_lifecycle/http.php"))
        .toml(&format!(
            "[supervisor]\nprocess_control_timeout_secs = {budget}\npidfile = 'server.pid'"
        ))
        .spawn()
}

#[test]
fn a_blocked_native_call_exits_within_the_shutdown_budget() {
    let mut srv = server(2);
    let mut conn = Conn::open(srv.addr, T).unwrap();
    conn.send(b"GET /?hold=1 HTTP/1.1\r\nHost: e2e\r\n\r\n")
        .unwrap();
    assert!(
        wait_log_contains(&srv, "lifecycle-hold-entered", T),
        "{}",
        diagnostics(&srv)
    );
    let start = Instant::now();
    send_ctrl_break(srv.pid());
    let status = srv.wait_exit(Duration::from_secs(5));
    assert_exit_code(status, 1, &srv);
    assert!(start.elapsed() < Duration::from_secs(5));
    assert!(
        srv.dir.join("server.pid").is_file(),
        "forced termination leaves the pidfile"
    );
}

#[test]
fn a_second_ctrl_break_forces_exit_during_drain_and_php_join() {
    for join in [false, true] {
        let mut srv = server(30);
        let mut conn = Conn::open(srv.addr, T).unwrap();
        if join {
            assert_eq!(
                http_get(srv.addr, "/?join=1", T).unwrap(),
                (200, b"done".to_vec())
            );
            assert!(wait_log_contains(&srv, "lifecycle-join-blocked", T));
        } else {
            conn.send(b"GET /?hold=1 HTTP/1.1\r\nHost: e2e\r\n\r\n")
                .unwrap();
            assert!(wait_log_contains(&srv, "lifecycle-hold-entered", T));
        }
        send_ctrl_break(srv.pid());
        assert!(wait_log_contains(&srv, DRAINING, T));
        if join {
            assert!(wait_log_contains(
                &srv,
                "drained cleanly; accept loop stopped",
                T
            ));
        }
        assert!(srv.try_status().is_none());
        send_ctrl_break(srv.pid());
        let status = srv.wait_exit(Duration::from_secs(3));
        assert_exit_code(status, 130, &srv);
        assert!(srv.dir.join("server.pid").is_file());
    }
}

#[test]
fn clean_shutdown_finishes_the_response_and_removes_the_locked_pidfile() {
    let mut srv = server(6);
    let pidfile = srv.dir.join("server.pid");
    assert_eq!(
        std::fs::read_to_string(&pidfile).unwrap().trim(),
        srv.pid().to_string()
    );
    let (status, log) = Spawn::http(Mode::Worker, fixture("windows_lifecycle/http.php"))
        .toml(&format!("[supervisor]\npidfile = '{}'", pidfile.display()))
        .boot_failure();
    assert_eq!(status.code(), Some(1), "{log}");
    assert!(pidfile.is_file());
    assert_eq!(
        http_get(srv.addr, "/", T).unwrap(),
        (200, b"ready".to_vec())
    );
    let mut conn = Conn::open(srv.addr, T).unwrap();
    conn.send(b"GET /?drain=1 HTTP/1.1\r\nHost: e2e\r\n\r\n")
        .unwrap();
    assert!(wait_log_contains(&srv, "lifecycle-drain-entered", T));
    send_ctrl_break(srv.pid());
    assert!(wait_log_contains(&srv, DRAINING, T));
    std::fs::write(srv.dir.join("http/release-drain"), b"go").unwrap();
    assert_eq!(conn.read_head(T).unwrap().0, 200);
    assert_eq!(conn.read_n(8, T).unwrap(), b"finished");
    let status = srv.wait_exit(T);
    assert_exit_code(status, 0, &srv);
    assert!(!pidfile.exists());
}

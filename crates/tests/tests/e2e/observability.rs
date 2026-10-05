use std::collections::BTreeMap;
use std::io::Write;
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};

use rapira_sapi::Mode;
use tests::wire::submit;
use tests::{Resp, drain_resp_deadline, req};

use crate::harness::{
    BOOT, Server, Spawn, diagnostics, fixture_path, free_port, http_get, http_raw,
    wait_log_contains,
};

const METRICS: &str = "[observability.metrics]\n[observability.probes]";
const REQUESTS: &str = "rapira_requests_total{pool=\"http\"}";
const QUEUED: &str = "rapira_requests_queued{pool=\"http\"}";
const RESTARTS: &str = "rapira_script_restarts_total{pool=\"http\"}";
const FAILED: &str = "rapira_requests_failed_total{pool=\"http\"}";
const FULL: &str = "rapira_requests_failed_on_full_queue_total{pool=\"http\"}";

fn spawn(fixture: &str, pool: &str, tables: &str) -> (Server, SocketAddr) {
    let addr = SocketAddr::from(([127, 0, 0, 1], free_port()));
    let srv = Spawn::http(Mode::Dispatcher, fixture_path(fixture))
        .http_pool(pool)
        .toml(&format!("[observability]\nlisten = \"{addr}\"\n{tables}"))
        .spawn();
    (srv, addr)
}

fn get(addr: SocketAddr, path: &str) -> Resp {
    let mut reply = submit(addr, req(path)).expect("connect to observability");
    drain_resp_deadline(&mut reply, Instant::now() + BOOT).expect("observability response")
}

fn scrape(addr: SocketAddr) -> BTreeMap<String, u64> {
    let resp = get(addr, "/metrics");
    assert_eq!(resp.status(), 200, "{}", resp.body_string());
    assert_eq!(
        resp.header("content-type").as_deref(),
        Some("text/plain; version=0.0.4; charset=utf-8")
    );
    tests::metrics::samples(&resp.body_string())
}

fn until(
    srv: &Server,
    addr: SocketAddr,
    predicate: impl Fn(&BTreeMap<String, u64>) -> bool,
) -> BTreeMap<String, u64> {
    let end = Instant::now() + BOOT;
    loop {
        let values = scrape(addr);
        if predicate(&values) {
            return values;
        }
        assert!(Instant::now() < end, "{values:?}\n{}", diagnostics(srv));
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn enabled_routes_and_methods() {
    struct Case {
        name: &'static str,
        tables: &'static str,
        path: &'static str,
        status: u16,
    }
    let cases = [
        Case {
            name: "live",
            tables: "[observability.probes]",
            path: "/livez",
            status: 200,
        },
        Case {
            name: "metrics disabled",
            tables: "[observability.probes]",
            path: "/metrics",
            status: 404,
        },
        Case {
            name: "probes disabled",
            tables: "[observability.metrics]",
            path: "/livez",
            status: 404,
        },
        Case {
            name: "unknown path",
            tables: METRICS,
            path: "/other",
            status: 404,
        },
    ];
    for case in cases {
        let (_srv, addr) = spawn("lifecycle/metrics-worker.php", "", case.tables);
        let resp = get(addr, case.path);
        assert_eq!(resp.status(), case.status, "{}", case.name);
        if case.status == 200 {
            assert_eq!(resp.body_string(), "ok\n");
            assert_eq!(
                resp.header("content-type").as_deref(),
                Some("text/plain; charset=utf-8")
            );
        }
        let (status, _) = http_raw(addr, b"POST /metrics HTTP/1.1\r\nHost: e2e\r\nContent-Length: 0\r\nConnection: close\r\n\r\n", BOOT).unwrap();
        assert_eq!(status, 404, "{}", case.name);
    }
}

#[test]
fn boot_and_retry_control_readiness_without_traffic() {
    struct Case {
        name: &'static str,
        fixture: &'static str,
        failed: bool,
    }
    let cases = [
        Case {
            name: "slow boot",
            fixture: "lifecycle/slow-boot-worker.php",
            failed: false,
        },
        Case {
            name: "failed boot",
            fixture: "lifecycle/dependency-boot-worker.php",
            failed: true,
        },
    ];
    for case in cases {
        let (srv, addr) = spawn(case.fixture, "", METRICS);
        if case.failed {
            assert!(wait_log_contains(&srv, "dependency down", BOOT));
        }
        let resp = get(addr, "/readyz");
        assert_eq!(
            (resp.status(), resp.body_string()),
            (503, "pool http: no ready worker\n".to_owned()),
            "{}",
            case.name
        );
        let values = scrape(addr);
        assert_eq!(
            values["rapira_workers{pool=\"http\",state=\"starting\"}"],
            1
        );
        if case.failed {
            std::fs::write(srv.dir.join("http/up.flag"), "").unwrap();
        }
        until(&srv, addr, |s| {
            s["rapira_workers{pool=\"http\",state=\"idle\"}"] == 1
        });
        assert_eq!(get(addr, "/readyz").status(), 200);
    }
}

#[test]
fn counters_survive_interpreter_recycling() {
    let (srv, addr) = spawn("lifecycle/metrics-worker.php", "max_requests = 2", METRICS);
    for _ in 0..3 {
        assert_eq!(http_get(srv.addr, "/", BOOT).unwrap().0, 200);
    }
    until(&srv, addr, |s| s[RESTARTS] >= 1);
    for _ in 0..2 {
        assert_eq!(http_get(srv.addr, "/", BOOT).unwrap().0, 200);
    }
    let values = until(&srv, addr, |s| s[REQUESTS] == 5);
    assert_eq!(values["rapira_workers_configured{pool=\"http\"}"], 1);
    assert_eq!(values["rapira_requests_failed_total{pool=\"http\"}"], 0);
    assert!(
        values
            .iter()
            .any(|(key, &value)| key.starts_with("rapira_build_info{") && value == 1)
    );
    assert!(
        values
            .keys()
            .all(|key| !key.starts_with("rapira_worker_exits_")
                && !key.starts_with("rapira_worker_rss_")
                && !key.starts_with("rapira_worker_pss_"))
    );
}

#[test]
fn a_shared_queue_counts_each_pending_request_once() {
    let addr = SocketAddr::from(([127, 0, 0, 1], free_port()));
    let srv = Spawn::http(Mode::Dispatcher, fixture_path("lifecycle/metrics-worker.php"))
        .processes(2, 0)
        .toml(&format!("[observability]\nlisten = \"{addr}\"\n{METRICS}\n[supervisor]\nprocess_control_timeout_secs = 1"))
        .spawn();
    let mut clients = Vec::new();
    for _ in 0..2 {
        let mut client = TcpStream::connect(srv.addr).unwrap();
        client
            .write_all(b"GET /hold HTTP/1.1\r\nHost: e2e\r\n\r\n")
            .unwrap();
        clients.push(client);
    }
    until(&srv, addr, |s| {
        s["rapira_workers{pool=\"http\",state=\"active\"}"] == 2
    });
    for _ in 0..3 {
        let mut client = TcpStream::connect(srv.addr).unwrap();
        client
            .write_all(b"GET / HTTP/1.1\r\nHost: e2e\r\n\r\n")
            .unwrap();
        clients.push(client);
    }
    until(&srv, addr, |s| s[QUEUED] == 3);
}

#[test]
fn a_failed_reboot_removes_readiness() {
    let (srv, addr) = spawn("lifecycle/reboot-fails-worker.php", "", METRICS);
    assert_eq!(http_get(srv.addr, "/", BOOT).unwrap().0, 200);
    assert!(wait_log_contains(&srv, "reboot failed", BOOT));
    let values = until(&srv, addr, |s| s[REQUESTS] == 1);
    assert_eq!(
        values["rapira_workers{pool=\"http\",state=\"starting\"}"],
        1
    );
    assert_eq!(get(addr, "/readyz").status(), 503);
    assert_eq!(http_get(srv.addr, "/", BOOT).unwrap().0, 503);
    let values = until(&srv, addr, |s| s[FAILED] == 1);
    assert_eq!(values[REQUESTS], 2);
    assert_eq!(
        values["rapira_workers{pool=\"http\",state=\"starting\"}"],
        1
    );
}

#[test]
fn full_queue_cancellation_and_timeout_have_distinct_totals() {
    let (srv, addr) = spawn(
        "lifecycle/metrics-worker.php",
        "",
        &format!("{METRICS}\n[supervisor]\nprocess_control_timeout_secs = 1"),
    );
    let _held = submit(srv.addr, req("/hold")).unwrap();
    until(&srv, addr, |s| {
        s["rapira_workers{pool=\"http\",state=\"active\"}"] == 1
    });
    let mut queued = Vec::new();
    for _ in 0..1024 {
        queued.push(submit(srv.addr, req("/")).unwrap());
    }
    until(&srv, addr, |s| s[QUEUED] == 1024);
    let cancelled = submit(srv.addr, req("/")).unwrap();
    until(&srv, addr, |s| s[QUEUED] == 1025);
    drop(cancelled);
    let values = until(&srv, addr, |s| s[QUEUED] == 1024 && s[FULL] == 1);
    assert_eq!((values[REQUESTS], values[FAILED]), (0, 0));
    let mut timed_out = submit(srv.addr, req("/")).unwrap();
    let response =
        drain_resp_deadline(&mut timed_out, Instant::now() + Duration::from_secs(40)).unwrap();
    assert_eq!(response.status(), 503);
    let values = until(&srv, addr, |s| s[FULL] == 2);
    assert_eq!(
        (values[REQUESTS], values[FAILED], values[QUEUED]),
        (1, 1, 1024)
    );
}

#[test]
fn readiness_checks_every_plugin_pool() {
    let addr = SocketAddr::from(([127, 0, 0, 1], free_port()));
    let srv = Spawn::http(
        Mode::Dispatcher,
        fixture_path("lifecycle/metrics-worker.php"),
    )
    .with_grpc(fixture_path("lifecycle/dependency-boot-worker.php"))
    .toml(&format!("[observability]\nlisten = \"{addr}\"\n{METRICS}"))
    .spawn();
    assert!(wait_log_contains(&srv, "dependency down", BOOT));
    until(&srv, addr, |s| {
        s["rapira_workers{pool=\"http\",state=\"idle\"}"] == 1
    });
    assert_eq!(
        get(addr, "/readyz").body_string(),
        "pool grpc: no ready worker\n"
    );
    std::fs::write(srv.dir.join("grpc/up.flag"), "").unwrap();
    until(&srv, addr, |s| {
        s["rapira_workers{pool=\"grpc\",state=\"idle\"}"] == 1
    });
    assert_eq!(get(addr, "/readyz").status(), 200);
}

#[test]
fn observability_keepalive_and_shutdown_are_bounded() {
    let (mut srv, addr) = spawn(
        "lifecycle/metrics-worker.php",
        "",
        "keepalive_timeout_secs = 1\n[observability.probes]",
    );
    let (status, _) = http_raw(
        addr,
        b"GET /livez HTTP/1.1\r\nHost: e2e\r\n\r\n",
        Duration::from_secs(5),
    )
    .unwrap();
    assert_eq!(status, 200);
    let _idle = TcpStream::connect(addr).unwrap();
    srv.stop();
}

#[test]
fn observability_bind_failure_stops_boot() {
    let addr = SocketAddr::from(([127, 0, 0, 1], free_port()));
    let (status, log) = Spawn::http(
        Mode::Dispatcher,
        fixture_path("lifecycle/metrics-worker.php"),
    )
    .http_listen(addr)
    .toml(&format!(
        "[observability]\nlisten = \"{addr}\"\n[observability.metrics]"
    ))
    .boot_failure();
    assert!(!status.success(), "{log}");
    assert!(log.contains(&format!("bind {addr}")), "{log}");
}

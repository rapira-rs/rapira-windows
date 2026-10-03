use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};

use crate::harness::*;
use rapira_sapi::Mode;

const REQ: Duration = Duration::from_secs(10);
const ECHO: &str = "shared/echo-worker.php";

#[test]
fn port_zero_serves_on_the_logged_port() {
    let srv = Spawn::http(Mode::Dispatcher, fixture_path(ECHO))
        .http_listen(([127, 0, 0, 1], 0).into())
        .with_grpc(fixture_path("grpc/echo-worker.php"))
        .spawn();
    let end = Instant::now() + BOOT;
    let addr: SocketAddr = loop {
        let log = std::fs::read_to_string(srv.log_file()).unwrap_or_default();
        if let Some(addr) = log.lines().find_map(|line| {
            line.split_once("http: prepared listener on ")
                .map(|(_, a)| a)
        }) {
            break addr.trim().parse().unwrap();
        }
        assert!(Instant::now() < end, "{log}");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_ne!(addr.port(), 0);
    assert_eq!(http_get(addr, "/", REQ).unwrap().0, 200);
}

#[test]
fn two_pools_on_one_address_fail_before_php_starts() {
    let addr = SocketAddr::from(([127, 0, 0, 1], free_port()));
    let (status, log) = Spawn::http(Mode::Dispatcher, fixture_path(ECHO))
        .http_listen(addr)
        .with_grpc(fixture_path("grpc/echo-worker.php"))
        .grpc_listen(addr)
        .boot_failure();
    assert!(!status.success(), "{log}");
    assert!(
        log.contains("plugin grpc: prepare failed") && log.contains(&format!("bind {addr}")),
        "{log}"
    );
    assert!(!log.contains("worker thread"), "{log}");
}

#[test]
fn another_process_cannot_take_a_live_listener() {
    let first = Spawn::http(Mode::Dispatcher, fixture_path(ECHO)).spawn();
    let (status, log) = Spawn::http(Mode::Dispatcher, fixture_path(ECHO))
        .http_listen(first.addr)
        .boot_failure();
    assert!(!status.success(), "{log}");
    assert_eq!(http_get(first.addr, "/", REQ).unwrap().0, 200);
}

#[test]
fn queued_connections_survive_each_accept() {
    let srv = spawn_with_config(ECHO, 1, "");
    let mut clients: Vec<_> = (0..16)
        .map(|_| TcpStream::connect_timeout(&srv.addr, REQ).unwrap())
        .collect();
    for client in &mut clients {
        client.set_read_timeout(Some(REQ)).unwrap();
        client
            .write_all(b"GET / HTTP/1.1\r\nHost: e2e\r\nConnection: close\r\n\r\n")
            .unwrap();
    }
    for client in &mut clients {
        let mut raw = String::new();
        client.read_to_string(&mut raw).unwrap();
        assert!(raw.starts_with("HTTP/1.1 200 "), "{raw}");
    }
}

//! Helpers for the rapira-windows smoke test: locate the built `rapira.exe`, reserve a
//! loopback port, wait for the server to accept, and perform one HTTP GET. std only.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Absolute path to a PHP fixture shipped with this crate (robust to the test's cwd).
pub fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(name)
}

/// Absolute path to the `rapira` binary built by this workspace. The test binary lives at
/// `target\<profile>\deps\<name>-<hash>.exe`; the bin is its sibling two levels up.
pub fn rapira_bin() -> PathBuf {
    let mut p = std::env::current_exe().expect("current_exe");
    p.pop(); // drop the test-binary file name -> ...\deps
    if p.file_name().is_some_and(|n| n == "deps") {
        p.pop(); // ...\deps -> ...\<profile>
    }
    let exe = p.join(format!("rapira{}", std::env::consts::EXE_SUFFIX));
    assert!(
        exe.is_file(),
        "rapira binary not found at {} — run `cargo build --bin rapira` first",
        exe.display()
    );
    exe
}

/// Reserve an unused loopback TCP port by binding :0 and releasing it.
pub fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind 127.0.0.1:0")
        .local_addr()
        .expect("local_addr")
        .port()
}

/// Poll until something accepts on `addr`, or `timeout` elapses.
pub fn wait_ready(addr: &str, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if TcpStream::connect(addr).is_ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("server never accepted on {addr} within {timeout:?}");
}

/// One HTTP/1.1 GET over a fresh connection. Returns `(status, body)`.
pub fn http_get(addr: &str, path: &str) -> (u16, String) {
    let mut s = TcpStream::connect(addr).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(10))).ok();
    write!(
        s,
        "GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n"
    )
    .expect("write request");
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).expect("read response");
    let text = String::from_utf8_lossy(&raw);
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .unwrap_or(0);
    (status, body.to_string())
}

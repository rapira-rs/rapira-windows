//! End-to-end smoke test: boot `rapira.exe serve` on a loopback port with a resident PHP
//! worker, send one HTTP request through the pingora front, assert the PHP response, then
//! terminate the process tree. Windows-only (the product's only target).

#![cfg(windows)]

use std::os::windows::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use integration_tests::{fixture, free_port, http_get, rapira_bin, wait_ready};

// Give the child its own process group so a stray Ctrl-C in the test runner cannot reach it,
// and so `taskkill /T` targets only rapira's tree.
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

/// Always reap the spawned server, even if an assertion panics.
struct Reaper(Child);
impl Drop for Reaper {
    fn drop(&mut self) {
        let _ = Command::new("taskkill")
            .args(["/PID", &self.0.id().to_string(), "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let _ = self.0.wait();
    }
}

#[test]
fn serve_worker_responds_over_http() {
    let port = free_port();
    let addr = format!("127.0.0.1:{port}");

    let child = Command::new(rapira_bin())
        .arg("serve")
        .arg("--listen")
        .arg(&addr)
        .arg("--threads")
        .arg("1")
        .arg(fixture("worker.php"))
        .env("RUST_LOG", "info")
        .creation_flags(CREATE_NEW_PROCESS_GROUP)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn rapira.exe (is php8ts.dll on PATH?)");
    let _reaper = Reaper(child);

    wait_ready(&addr, Duration::from_secs(30));

    let (status, body) = http_get(&addr, "/?name=windows");
    assert_eq!(status, 200, "unexpected status; body = {body:?}");
    assert!(
        body.contains("Hello from worker, windows!"),
        "unexpected body: {body:?}"
    );
}

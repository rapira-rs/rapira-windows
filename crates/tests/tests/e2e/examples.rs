use std::time::Duration;

use crate::harness::{http_get_raw, spawn_example};

const T: Duration = Duration::from_secs(10);

/// The Fiber example finishes the active exchange before it receives the next request.
#[test]
fn fiber_example_completes_the_stream() {
    let srv = spawn_example("dispatcher-async.php", 1);
    let raw = http_get_raw(srv.addr, "/stream", &[], T).expect("stream response");
    let response = String::from_utf8_lossy(&raw);

    assert!(response.starts_with("HTTP/1.1 200 "), "{response}");
    assert!(response.contains("stream done\n"), "{response}");
    assert!(response.ends_with("0\r\n\r\n"), "{response}");
}

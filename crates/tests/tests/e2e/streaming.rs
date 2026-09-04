use std::time::{Duration, Instant};

use crate::harness::{Conn, http_get_raw, spawn_with_config, wait_log_contains};

const T: Duration = Duration::from_secs(10);

/// `flush()` sends the head at least 400 ms before the first event.
#[test]
fn sse_head_and_events_reach_the_wire_incrementally() {
    let srv = spawn_with_config("lifecycle/stream-worker.php", 1, "");
    let mut c = Conn::open(srv.addr, T).expect("connect");
    c.send(b"GET /?probe=sse HTTP/1.1\r\nHost: e2e\r\nConnection: close\r\n\r\n")
        .expect("send");

    let started = Instant::now();
    let (status, fields) = c.read_head(T).expect("flushed head");
    assert_eq!(status, 200);
    assert!(
        started.elapsed() < Duration::from_millis(350),
        "the head must beat the 400ms-late first event (took {:?})",
        started.elapsed()
    );
    assert!(
        fields
            .iter()
            .any(|(k, v)| k == "transfer-encoding" && v == "chunked"),
        "flush costs the length: chunked framing expected, got {fields:?}"
    );

    c.read_body_until(b"data: one", T).expect("first event");
    c.read_body_until(b"data: two", T).expect("second event");
    let rest = c.read_remaining(T).expect("clean end");
    assert!(
        String::from_utf8_lossy(&rest).ends_with("0\r\n\r\n"),
        "the chunked terminator must close the stream"
    );
}

/// A chunked stream keeps the connection reusable for a second request.
#[test]
fn chunked_stream_preserves_keepalive() {
    let srv = spawn_with_config("lifecycle/stream-worker.php", 1, "");
    let mut c = Conn::open(srv.addr, T).expect("connect");

    c.send(b"GET /?probe=chunks HTTP/1.1\r\nHost: e2e\r\n\r\n")
        .expect("send");
    let (status, fields) = c.read_head(T).expect("head");
    assert_eq!(status, 200);
    assert!(
        fields
            .iter()
            .any(|(k, v)| k == "transfer-encoding" && v == "chunked"),
        "{fields:?}"
    );
    c.read_body_until(b"0\r\n\r\n", T).expect("terminator");

    c.send(b"GET /?probe=chunks HTTP/1.1\r\nHost: e2e\r\nConnection: close\r\n\r\n")
        .expect("second request on the same connection");
    let (status, _) = c.read_head(T).expect("reused connection must serve");
    assert_eq!(status, 200);
}

/// The HTTP server discards an interim 1xx response from PHP. The first received head block is the final 200 response, and the other response data remains unchanged.
#[test]
fn interim_heads_never_reach_the_wire() {
    let srv = spawn_with_config("lifecycle/stream-worker.php", 1, "");
    let mut c = Conn::open(srv.addr, T).expect("connect");
    c.send(b"GET /?probe=interim HTTP/1.1\r\nHost: e2e\r\nConnection: close\r\n\r\n")
        .expect("send");

    let (status, _) = c.read_head(T).expect("final head");
    assert_eq!(status, 200, "the first head block must be the final one");
    c.read_body_until(b"hello", T).expect("body");
}

/// An HTTP/1.0 client receives framing delimited by connection close and does not receive chunked framing.
#[test]
fn http10_gets_no_interim_and_no_chunked() {
    let srv = spawn_with_config("lifecycle/stream-worker.php", 1, "");

    let mut c = Conn::open(srv.addr, T).expect("connect");
    c.send(b"GET /?probe=interim HTTP/1.0\r\n\r\n")
        .expect("send");
    let (status, fields) = c.read_head(T).expect("head");
    assert_eq!(status, 200, "the interim head must be dropped for 1.0");
    assert!(
        !fields.iter().any(|(k, _)| k == "transfer-encoding"),
        "no chunked toward a 1.0 client: {fields:?}"
    );
    let rest = c.read_remaining(T).expect("close-delimited body");
    assert_eq!(rest, b"hello");
}

/// A declared length must not delay the head indefinitely. The time limit applies to combining the head with the first chunk.
#[test]
fn declared_length_head_beats_a_slow_body() {
    let srv = spawn_with_config("lifecycle/stream-worker.php", 1, "");
    let mut c = Conn::open(srv.addr, T).expect("connect");
    c.send(b"GET /?probe=cl-slow-body HTTP/1.1\r\nHost: e2e\r\nConnection: close\r\n\r\n")
        .expect("send");

    let started = Instant::now();
    let (status, fields) = c.read_head(T).expect("head");
    assert_eq!(status, 200);
    assert!(
        started.elapsed() < Duration::from_millis(1000),
        "the head must beat the 2s-late body (took {:?})",
        started.elapsed()
    );
    assert!(
        fields
            .iter()
            .any(|(k, v)| k == "content-length" && v == "5"),
        "the declared length still frames the response: {fields:?}"
    );
    c.read_body_until(b"01234", T).expect("body");
}

/// A body longer than the declared Content-Length is truncated to that length so the connection framing remains valid for reuse.
#[test]
fn content_length_exceeded_serves_the_declared_prefix() {
    let srv = spawn_with_config("lifecycle/stream-worker.php", 1, "");
    let raw = http_get_raw(srv.addr, "/?probe=cl-exceeded", &[], T).expect("response");
    let text = String::from_utf8_lossy(&raw);
    assert!(
        text.to_ascii_lowercase().contains("content-length: 5"),
        "declared length must be honoured: {text}"
    );
    assert!(
        text.ends_with("\r\n\r\n01234"),
        "exactly the fitting prefix: {text}"
    );
}

/// A client that disconnects during a stream causes `WorkDiscardedException` in the worker.
#[test]
fn client_abort_discards_the_unit() {
    let srv = spawn_with_config("lifecycle/stream-worker.php", 1, "");
    let mut c = Conn::open(srv.addr, T).expect("connect");
    c.send(b"GET /?probe=discard HTTP/1.1\r\nHost: e2e\r\n\r\n")
        .expect("send");
    let (status, _) = c.read_head(T).expect("flushed head");
    assert_eq!(status, 200);
    c.abandon();

    assert!(
        wait_log_contains(&srv, "WorkDiscardedException", T),
        "the worker must observe the abort on its next write"
    );
}

/// sendFile streams the file from disk with the actual Content-Length. PHP does not store the bytes.
#[test]
fn sendfile_streams_from_disk_with_length() {
    let srv = spawn_with_config("lifecycle/stream-worker.php", 1, "");
    let payload = srv.dir.join("payload.bin");
    std::fs::write(&payload, vec![b'z'; 100_000]).expect("payload");

    let mut c = Conn::open(srv.addr, T).expect("connect");
    c.send(
        format!(
            "GET /?probe=sendfile HTTP/1.1\r\nHost: e2e\r\nx-path: {}\r\nConnection: close\r\n\r\n",
            payload.display()
        )
        .as_bytes(),
    )
    .expect("send");
    let (status, fields) = c.read_head(T).expect("head");
    assert_eq!(status, 200);
    assert!(
        fields
            .iter()
            .any(|(k, v)| k == "content-length" && v == "100000"),
        "the file length is known up front: {fields:?}"
    );
    let rest = c.read_remaining(T).expect("body");
    assert_eq!(rest.len(), 100_000);
    assert!(rest.iter().all(|&b| b == b'z'), "file bytes intact");
}

/// The HTTP/1 path discards trailers. The final response has no trailer bytes, and the connection remains reusable.
#[test]
fn trailers_are_dropped_on_h1_and_the_response_ends_cleanly() {
    let srv = spawn_with_config("lifecycle/stream-worker.php", 1, "");
    let mut c = Conn::open(srv.addr, T).expect("connect");
    c.send(b"GET /?probe=trailers HTTP/1.1\r\nHost: e2e\r\n\r\n")
        .expect("send");

    let (status, fields) = c.read_head(T).expect("head");
    assert_eq!(status, 200);
    assert!(
        fields
            .iter()
            .any(|(k, v)| k == "transfer-encoding" && v == "chunked"),
        "{fields:?}"
    );
    c.read_body_until(b"payload", T).expect("body");
    c.read_body_until(b"0\r\n\r\n", T)
        .expect("a clean terminator with no trailer section");

    c.send(b"GET /?probe=chunks HTTP/1.1\r\nHost: e2e\r\nConnection: close\r\n\r\n")
        .expect("second request");
    let (status, _) = c.read_head(T).expect("reused connection must serve");
    assert_eq!(status, 200);
}

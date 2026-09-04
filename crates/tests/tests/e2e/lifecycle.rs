use crate::harness::*;
use std::collections::BTreeMap;
use std::net::TcpListener;
use std::time::{Duration, Instant};

#[test]
fn static_pool_starts_n_threads() {
    let srv = spawn_with_config("shared/echo-worker.php", 3, "");
    wait_workers(
        &srv,
        Duration::from_secs(20),
        "3 interpreter threads",
        |p| p.len() == 3,
    );
    let (code, _) = http_get(srv.addr, "/", Duration::from_secs(10)).expect("GET /");
    assert_eq!(
        code,
        200,
        "pool should serve once up\n{}",
        diagnostics(&srv)
    );
}

#[test]
fn http_round_trip() {
    let srv = spawn_with_config("shared/echo-worker.php", 1, "");
    wait_workers(&srv, Duration::from_secs(20), "1 worker", |p| p.len() == 1);
    for _ in 0..2 {
        let (code, body) =
            http_get(srv.addr, "/?from=e2e", Duration::from_secs(10)).expect("GET /?from=e2e");
        assert_eq!(code, 200, "\n{}", diagnostics(&srv));
        assert!(
            body.starts_with(b"ok:"),
            "body should start with ok:, got {:?}",
            String::from_utf8_lossy(&body)
        );
    }
}

/// Worker mode startup that does not call handle_request() must fail server startup. The server must not wait indefinitely or continue to return 503.
#[test]
fn worker_bootstrap_that_never_serves_failboots() {
    let mut srv = spawn_with_config("lifecycle/never-loop-worker.php", 1, "mode = \"worker\"\n");
    let addr = srv.addr;
    let end = Instant::now() + Duration::from_secs(60);
    let status = loop {
        if let Some(st) = srv.try_status() {
            break Some(st);
        }
        if Instant::now() >= end {
            panic!("server never exited\n{}", diagnostics(&srv));
        }
        let _ = http_get(addr, "/", Duration::from_secs(2));
        std::thread::sleep(Duration::from_millis(100));
    };
    assert_exit_code(status, 70, &srv);
}

/// A client that disconnects while the handler runs must not stop the worker. The abort recycles the cycle, and the worker processes the next request.
#[test]
fn worker_survives_client_abandon() {
    let srv = spawn_with_config("lifecycle/hold-worker.php", 1, "mode = \"worker\"\n");
    let mut c = Conn::open(srv.addr, Duration::from_secs(10)).expect("connect");
    c.send(b"GET / HTTP/1.1\r\nHost: e2e\r\n\r\n")
        .expect("send");
    assert!(
        wait_log_contains(&srv, "held", Duration::from_secs(10)),
        "\n{}",
        diagnostics(&srv)
    );
    c.abandon();

    let (code, body) = http_get(srv.addr, "/?probe=1", Duration::from_secs(10)).expect("GET");
    assert_eq!(code, 200, "\n{}", diagnostics(&srv));
    assert_eq!(body, b"ok", "\n{}", diagnostics(&srv));
}

/// An HTTP field that php-src permits but the HTTP server cannot represent must remove only that field from the response.
#[test]
fn unrepresentable_header_still_serves_the_response() {
    let srv = spawn_with_config("lifecycle/bad-header-worker.php", 1, "mode = \"worker\"\n");
    wait_workers(&srv, Duration::from_secs(20), "1 worker", |p| p.len() == 1);
    let (code, body) = http_get(srv.addr, "/", Duration::from_secs(10)).expect("GET /");
    assert_eq!(code, 201, "\n{}", diagnostics(&srv));
    assert_eq!(body, b"body", "\n{}", diagnostics(&srv));
}

/// php-src must receive the exact multipart boundary bytes. If decoding changes the boundary, rfc1867 searches for bytes that the body does not contain and omits the upload.
#[test]
fn non_utf8_multipart_boundary_uploads() {
    let srv = spawn_with_config("lifecycle/upload-worker.php", 1, "mode = \"worker\"\n");
    wait_workers(&srv, Duration::from_secs(20), "1 worker", |p| p.len() == 1);
    let boundary: &[u8] = b"RAP\xff\xfeIRA";
    let mut body = Vec::new();
    body.extend_from_slice(b"--");
    body.extend_from_slice(boundary);
    body.extend_from_slice(b"\r\nContent-Disposition: form-data; name=\"file\"; filename=\"foo.txt\"\r\nContent-Type: text/plain\r\n\r\nbar\r\n--");
    body.extend_from_slice(boundary);
    body.extend_from_slice(b"--\r\n");
    let mut ctype = b"multipart/form-data; boundary=".to_vec();
    ctype.extend_from_slice(boundary);

    let (code, out) =
        http_post(srv.addr, "/", &ctype, &body, Duration::from_secs(10)).expect("POST /");
    assert_eq!(code, 200, "\n{}", diagnostics(&srv));
    let out = String::from_utf8_lossy(&out);
    let tmp = out.strip_prefix("foo.txt|0|bar|").unwrap_or_else(|| {
        panic!("upload must parse (got {out:?})\n{}", diagnostics(&srv));
    });
    assert!(
        !std::path::Path::new(tmp).exists(),
        "upload temp file {tmp} must be cleaned up\n{}",
        diagnostics(&srv)
    );
}

/// PHP receives repeated fields as one value. Most fields use a comma-separated list, and Cookie uses `"; "`.
#[test]
fn repeated_request_fields_reach_php_combined() {
    let srv = spawn_with_config(
        "lifecycle/repeated-headers-worker.php",
        1,
        "mode = \"worker\"\n",
    );
    wait_workers(&srv, Duration::from_secs(20), "1 worker", |p| p.len() == 1);
    let (code, body) = http_get_with_headers(
        srv.addr,
        "/",
        &[
            ("Cookie", "a=1"),
            ("Cookie", "b=2"),
            ("X-Forwarded-For", "203.0.113.7"),
            ("X-Forwarded-For", "10.0.0.1"),
        ],
        Duration::from_secs(10),
    )
    .expect("GET /");
    assert_eq!(code, 200, "\n{}", diagnostics(&srv));
    assert_eq!(
        String::from_utf8_lossy(&body),
        "1,2\na=1; b=2\n203.0.113.7, 10.0.0.1\n",
        "\n{}",
        diagnostics(&srv)
    );
}

/// An HTTP field name with `_` or `.` must not modify the CGI variable for the corresponding name with `-`. PHP changes `.` to `_` when it registers a variable.
#[test]
fn alias_names_never_reach_a_cgi_variable() {
    let srv = spawn_with_config(
        "lifecycle/repeated-headers-worker.php",
        1,
        "mode = \"worker\"\n",
    );
    wait_workers(&srv, Duration::from_secs(20), "1 worker", |p| p.len() == 1);
    let (code, body) = http_get_with_headers(
        srv.addr,
        "/",
        &[
            ("X_Forwarded_For", "1.2.3.4"),
            ("X.Forwarded.For", "5.6.7.8"),
        ],
        Duration::from_secs(10),
    )
    .expect("GET /");
    assert_eq!(code, 200, "\n{}", diagnostics(&srv));
    assert_eq!(
        String::from_utf8_lossy(&body),
        "-,-\n-\n-\n",
        "no alias may reach HTTP_X_FORWARDED_FOR\n{}",
        diagnostics(&srv)
    );
}

/// `reject` sends a 400 response. Field name validation runs before dispatch, so PHP does not receive the request.
#[test]
fn reject_policy_answers_400_for_an_alias_name() {
    let srv = spawn_with_http_extra(
        "shared/echo-worker.php",
        1,
        "unsafe_field_names = \"reject\"\n",
    );
    wait_workers(&srv, Duration::from_secs(20), "1 worker", |p| p.len() == 1);

    let (code, _) = http_get_with_headers(
        srv.addr,
        "/",
        &[("X_Forwarded_For", "1.2.3.4")],
        Duration::from_secs(10),
    )
    .expect("GET / with an alias name");
    assert_eq!(code, 400, "\n{}", diagnostics(&srv));

    let (code, _) = http_get_with_headers(
        srv.addr,
        "/",
        &[("X-Forwarded-For", "203.0.113.7")],
        Duration::from_secs(10),
    )
    .expect("GET / with a safe name");
    assert_eq!(code, 200, "\n{}", diagnostics(&srv));
}

/// More than one `Host` field causes a 400 response. The HTTP/1 parser preserves repeated fields, so this server performs the rejection. RFC 9112 section 3.2: https://www.rfc-editor.org/rfc/rfc9112#section-3.2
#[test]
fn a_second_host_field_line_answers_400() {
    let srv = spawn_with_config("lifecycle/fidelity-worker.php", 1, "");
    wait_workers(&srv, Duration::from_secs(20), "1 worker", |p| p.len() == 1);

    let (code, _) = http_get_with_headers(
        srv.addr,
        "/",
        &[("Host", "evil.example")],
        Duration::from_secs(10),
    )
    .expect("GET / with two Host field lines");
    assert_eq!(code, 400, "\n{}", diagnostics(&srv));
}

/// CONNECT returns 501 before dispatch. A 2xx response from PHP would change the connection to a tunnel. A 405 response requires an Allow field under RFC 9110 section 15.5.6: https://www.rfc-editor.org/rfc/rfc9110#section-15.5.6
#[test]
fn connect_answers_501() {
    let srv = spawn_with_config("lifecycle/fidelity-worker.php", 1, "");
    wait_workers(&srv, Duration::from_secs(20), "1 worker", |p| p.len() == 1);

    let raw = http_raw_bytes(
        srv.addr,
        b"CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\n\r\n",
        Duration::from_secs(10),
    )
    .expect("CONNECT request");
    let head = String::from_utf8_lossy(&raw);
    assert!(
        head.starts_with("HTTP/1.1 501"),
        "got {:?}\n{}",
        head.lines().next().unwrap_or(""),
        diagnostics(&srv)
    );
}

/// When a streamed body has no declared length, the accumulation loop returns 413 when the body exceeds max_body_size.
#[test]
fn chunked_body_over_the_cap_answers_413() {
    let srv = spawn_with_http_extra("lifecycle/fidelity-worker.php", 1, "max_body_size_mb = 1\n");
    wait_workers(&srv, Duration::from_secs(20), "1 worker", |p| p.len() == 1);

    let mut c = Conn::open(srv.addr, Duration::from_secs(10)).expect("connect");
    c.send(b"POST / HTTP/1.1\r\nHost: e2e\r\nTransfer-Encoding: chunked\r\n\r\n")
        .expect("head");
    let chunk = vec![b'a'; 64 * 1024];
    let size_line = format!("{:x}\r\n", chunk.len());
    // The last of 17 chunks of 64 KiB exceeds the 1 MiB limit. The server returns 413 before the client sends a terminal chunk.
    for _ in 0..17 {
        if c.send(size_line.as_bytes()).is_err()
            || c.send(&chunk).is_err()
            || c.send(b"\r\n").is_err()
        {
            break;
        }
    }
    let (status, _) = c.read_head(Duration::from_secs(10)).expect("413 head");
    assert_eq!(status, 413, "\n{}", diagnostics(&srv));
}

/// A chunked body below the limit dispatches normally. This verifies that the size check does not reject a valid body.
#[test]
fn chunked_body_under_the_cap_is_served() {
    let srv = spawn_with_http_extra("lifecycle/fidelity-worker.php", 1, "max_body_size_mb = 1\n");
    wait_workers(&srv, Duration::from_secs(20), "1 worker", |p| p.len() == 1);

    let mut c = Conn::open(srv.addr, Duration::from_secs(10)).expect("connect");
    c.send(
        b"POST / HTTP/1.1\r\nHost: e2e\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
    )
    .expect("head");
    c.send(b"5\r\nhello\r\n0\r\n\r\n").expect("body");
    let (status, _) = c.read_head(Duration::from_secs(10)).expect("head");
    assert_eq!(status, 200, "\n{}", diagnostics(&srv));
    c.read_body_until(b"ok", Duration::from_secs(10))
        .expect("body");
}

/// The server sends the interim 100 response when it first polls the body, after admission succeeds.
#[test]
fn expect_100_continue_gets_the_interim_then_the_response() {
    let srv = spawn_with_config("lifecycle/fidelity-worker.php", 1, "");
    wait_workers(&srv, Duration::from_secs(20), "1 worker", |p| p.len() == 1);

    let mut c = Conn::open(srv.addr, Duration::from_secs(10)).expect("connect");
    c.send(b"POST / HTTP/1.1\r\nHost: e2e\r\nExpect: 100-continue\r\nContent-Length: 5\r\nConnection: close\r\n\r\n")
        .expect("head");
    let (status, _) = c.read_head(Duration::from_secs(10)).expect("interim head");
    assert_eq!(status, 100, "\n{}", diagnostics(&srv));
    c.send(b"hello").expect("body");
    let (status, _) = c.read_head(Duration::from_secs(10)).expect("final head");
    assert_eq!(status, 200, "\n{}", diagnostics(&srv));
}

/// A refused request must not receive a 100 response because admission rejects it before polling the body.
#[test]
fn expect_100_continue_is_skipped_when_the_request_is_refused() {
    let srv = spawn_with_http_extra("lifecycle/fidelity-worker.php", 1, "max_body_size_mb = 1\n");
    wait_workers(&srv, Duration::from_secs(20), "1 worker", |p| p.len() == 1);

    let mut c = Conn::open(srv.addr, Duration::from_secs(10)).expect("connect");
    c.send(
        b"POST / HTTP/1.1\r\nHost: e2e\r\nExpect: 100-continue\r\nContent-Length: 2097152\r\n\r\n",
    )
    .expect("head");
    let (status, _) = c
        .read_head(Duration::from_secs(10))
        .expect("direct refusal");
    assert_eq!(
        status,
        413,
        "no interim before the refusal\n{}",
        diagnostics(&srv)
    );
}

/// The server cannot send a final 1xx response from PHP. hyper would change it to 500 and close the connection with an error, so the server returns 502. https://github.com/hyperium/hyper/blob/6371cd425017155f7fbecef0e57b218edbe6a93a/src/proto/h1/role.rs#L392-L408
#[test]
fn final_interim_status_becomes_502() {
    let srv = spawn_with_config("lifecycle/status-1xx-worker.php", 1, "mode = \"worker\"\n");
    wait_workers(&srv, Duration::from_secs(20), "1 worker", |p| p.len() == 1);

    let raw = http_raw_bytes(
        srv.addr,
        b"GET / HTTP/1.1\r\nHost: e2e\r\nConnection: close\r\n\r\n",
        Duration::from_secs(10),
    )
    .expect("response");
    let head = String::from_utf8_lossy(&raw);
    assert!(
        head.starts_with("HTTP/1.1 502"),
        "got {:?}\n{}",
        head.lines().next().unwrap_or(""),
        diagnostics(&srv)
    );
}

/// `header("Status: 404")` must set the response code. php-src `sapi_header_op` preserves the field, and the origin server converts it under RFC 3875 section 6.3.3: https://www.rfc-editor.org/rfc/rfc3875#section-6.3.3
#[test]
fn status_field_sets_the_code_and_never_reaches_the_client() {
    let srv = spawn_with_config(
        "lifecycle/status-header-worker.php",
        1,
        "mode = \"worker\"\n",
    );
    wait_workers(&srv, Duration::from_secs(20), "1 worker", |p| p.len() == 1);
    let raw = http_get_raw(srv.addr, "/", &[], Duration::from_secs(10)).expect("GET /");
    let text = String::from_utf8_lossy(&raw).to_ascii_lowercase();

    assert!(
        text.starts_with("http/1.1 404"),
        "Status: must set the response code (got {:?})\n{}",
        text.lines().next().unwrap_or(""),
        diagnostics(&srv)
    );
    assert!(
        !text.contains("\r\nstatus:"),
        "Status: must not reach the client\n{}",
        diagnostics(&srv)
    );
    assert!(
        text.contains("x-keep: kept"),
        "other fields must survive\n{}",
        diagnostics(&srv)
    );
}

/// Checks request values through a socket. PHP receives repeated field lines as a list, receivedAt is a valid receive timestamp, and an HTTP/1.1 request without Host receives 400 before dispatch. RFC 9112 section 3.2: https://www.rfc-editor.org/rfc/rfc9112#section-3.2
#[test]
fn dispatcher_request_fidelity_over_the_wire() {
    let srv = spawn_with_config("lifecycle/fidelity-worker.php", 1, "");

    let before = std::time::UNIX_EPOCH.elapsed().unwrap().as_secs_f64();
    let (code, body) = http_get_with_headers(
        srv.addr,
        "/?probe=headers",
        &[
            ("X-Probe", "one"),
            ("X-Probe", "two"),
            ("x_forwarded_for", "1.2.3.4"),
        ],
        Duration::from_secs(10),
    )
    .expect("GET headers probe");
    assert_eq!(code, 200, "\n{}", diagnostics(&srv));
    assert_eq!(
        String::from_utf8_lossy(&body),
        "x-probe=one|two\nx_forwarded_for=1.2.3.4"
    );

    let (code, body) =
        http_get(srv.addr, "/?probe=received", Duration::from_secs(10)).expect("GET received");
    assert_eq!(code, 200);
    let after = std::time::UNIX_EPOCH.elapsed().unwrap().as_secs_f64();
    let received: f64 = String::from_utf8_lossy(&body)
        .trim_start_matches("received=")
        .parse()
        .expect("receivedAt is a float");
    assert!(
        received >= before && received <= after,
        "receivedAt {received} outside [{before}, {after}]"
    );

    let (code, _) = http_raw(
        srv.addr,
        b"GET / HTTP/1.1\r\nConnection: close\r\n\r\n",
        Duration::from_secs(10),
    )
    .expect("Host-less request");
    assert_eq!(code, 400, "missing Host on HTTP/1.1 must answer 400");
}

/// Checks a response head from PHP. The response contains the status line, one field line for each list value, framing from the HTTP server, and no body or Content-Length for `HEAD`. RFC 9110 section 8.6: https://www.rfc-editor.org/rfc/rfc9110#section-8.6
#[test]
fn dispatcher_write_head_reaches_the_wire() {
    let srv = spawn_with_config("lifecycle/fidelity-worker.php", 1, "");
    wait_workers(&srv, Duration::from_secs(20), "1 worker", |p| p.len() == 1);

    let raw = http_get_raw(srv.addr, "/?probe=head", &[], Duration::from_secs(10))
        .expect("GET head probe");
    let text = String::from_utf8_lossy(&raw).to_ascii_lowercase();
    assert!(
        text.starts_with("http/1.1 201"),
        "got {:?}\n{}",
        text.lines().next().unwrap_or(""),
        diagnostics(&srv)
    );
    assert_eq!(
        text.matches("\r\nx-a: ").count(),
        2,
        "one field line per list value\n{}",
        diagnostics(&srv)
    );
    assert!(
        text.contains("\r\ncontent-length: 999\r\n"),
        "the declared content-length must be honoured\n{}",
        diagnostics(&srv)
    );
    assert!(text.ends_with("body"), "{text:?}\n{}", diagnostics(&srv));

    let raw = http_raw_bytes(
        srv.addr,
        b"HEAD /?probe=head HTTP/1.1\r\nHost: e2e\r\nConnection: close\r\n\r\n",
        Duration::from_secs(10),
    )
    .expect("HEAD head probe");
    let text = String::from_utf8_lossy(&raw).to_ascii_lowercase();
    assert!(
        text.starts_with("http/1.1 201"),
        "got {:?}\n{}",
        text.lines().next().unwrap_or(""),
        diagnostics(&srv)
    );
    assert!(
        !text.contains("\r\ncontent-length:"),
        "no content-length on a HEAD response\n{}",
        diagnostics(&srv)
    );
    assert!(
        text.ends_with("\r\n\r\n"),
        "no body bytes on a HEAD response\n{}",
        diagnostics(&srv)
    );
}

/// RFC 9112 section 3.2.2 requires an origin server to ignore Host and use the host information from an absolute-form target.
/// https://www.rfc-editor.org/rfc/rfc9112#section-3.2.2
#[test]
fn absolute_form_target_overrides_host() {
    let srv = spawn_with_config("lifecycle/fidelity-worker.php", 1, "");
    wait_workers(&srv, Duration::from_secs(20), "1 worker", |p| p.len() == 1);

    let raw = http_raw_bytes(
        srv.addr,
        b"GET http://target.example/echo?probe=target HTTP/1.1\r\n\
          Host: spoofed.example\r\nConnection: close\r\n\r\n",
        Duration::from_secs(10),
    )
    .expect("absolute-form request");
    let text = String::from_utf8_lossy(&raw);
    assert!(
        text.contains("target=http://target.example/echo?probe=target"),
        "the target must keep the absolute form: {text}\n{}",
        diagnostics(&srv)
    );
    assert!(
        text.contains("authority=target.example"),
        "the target authority must replace Host: {text}\n{}",
        diagnostics(&srv)
    );
    assert!(
        text.contains("uri=http://target.example/echo?probe=target"),
        "the absolute uri must carry the target authority and the path: {text}\n{}",
        diagnostics(&srv)
    );
    assert!(
        text.contains("host=target.example"),
        "the Host field line must carry the effective authority: {text}\n{}",
        diagnostics(&srv)
    );
}

/// Checks host multipart parsing through HTTP. A non-UTF-8 boundary remains unchanged, finalization removes the spool file, invalid framing returns 400, and a file part above the limit returns 413.
#[test]
fn dispatcher_multipart_over_the_wire() {
    let srv = spawn_with_http_extra(
        "lifecycle/fidelity-worker.php",
        1,
        "[http.uploads]\nmax_file_size_mb = 1\n",
    );

    let ct = b"multipart/form-data; boundary=RAP\xff\xfeIRA".to_vec();
    let mut body = Vec::new();
    body.extend_from_slice(
        b"--RAP\xff\xfeIRA\r\ncontent-disposition: form-data; name=\"note\"\r\n\r\nhello\r\n",
    );
    body.extend_from_slice(b"--RAP\xff\xfeIRA\r\ncontent-disposition: form-data; name=\"f\"; filename=\"a.bin\"\r\n\r\nPAYLOAD\r\n");
    body.extend_from_slice(b"--RAP\xff\xfeIRA--");
    let (code, resp) = http_post(
        srv.addr,
        "/?probe=multipart",
        &ct,
        &body,
        Duration::from_secs(10),
    )
    .expect("multipart POST");
    let text = String::from_utf8_lossy(&resp).into_owned();
    assert_eq!(code, 200, "{text}\n{}", diagnostics(&srv));
    assert!(text.contains("field=note=hello"), "{text}");
    assert!(text.contains("file-content=PAYLOAD"), "{text}");
    let tmp = text
        .lines()
        .find_map(|l| l.strip_prefix("tmp="))
        .expect("tmp line");
    assert!(
        !std::path::Path::new(tmp).exists(),
        "spool file must be gone once the response arrived"
    );

    let (code, _) = http_post(
        srv.addr,
        "/?probe=multipart",
        b"multipart/form-data; boundary=B",
        b"no boundary line at all",
        Duration::from_secs(10),
    )
    .expect("malformed POST");
    assert_eq!(code, 400, "malformed multipart must answer 400");

    let mut big = Vec::new();
    big.extend_from_slice(
        b"--B\r\ncontent-disposition: form-data; name=\"f\"; filename=\"a\"\r\n\r\n",
    );
    big.extend(std::iter::repeat_n(b'x', 2 * 1024 * 1024));
    big.extend_from_slice(b"\r\n--B--");
    let (code, _) = http_post(
        srv.addr,
        "/?probe=multipart",
        b"multipart/form-data; boundary=B",
        &big,
        Duration::from_secs(10),
    )
    .expect("over-limit POST");
    assert_eq!(code, 413, "over-limit file part must answer 413");
}

const REQUEST: Duration = Duration::from_secs(10);
const EXIT: Duration = Duration::from_secs(15);
const DRAINING: &str = "shutdown event received; draining extensions";
const FIXTURE: &str = "lifecycle/windows-lifecycle-worker.php";

fn server_log(server: &Server) -> String {
    std::fs::read_to_string(server.dir.join("server.log")).expect("read server.log")
}

fn open_held_request(server: &Server) -> Conn {
    let mut connection = Conn::open(server.addr, REQUEST).expect("connect");
    connection
        .send(b"GET /?hold=1 HTTP/1.1\r\nHost: e2e\r\n\r\n")
        .expect("send held request");
    assert!(
        wait_log_contains(server, "lifecycle-hold-entered", REQUEST),
        "the request must be inside native sleep before stopping\n{}",
        diagnostics(server)
    );
    connection
}

#[test]
fn a_bootstrap_failure_exits_70_instead_of_shedding_forever() {
    let mut server = spawn_with_config(
        "lifecycle/fatal-worker.php",
        1,
        "mode = \"worker\"\n[supervisor]\nprocess_control_timeout_secs = 2\n",
    );
    let deadline = Instant::now() + EXIT;
    let status = loop {
        if let Some(status) = server.try_status() {
            break Some(status);
        }
        assert!(
            Instant::now() < deadline,
            "a failed bootstrap must stop the server within {EXIT:?}\n{}",
            diagnostics(&server)
        );
        // A failed cycle rejects one queued request before another attempt. Reach the threshold of five failures. PHP must not process a request in this test.
        if let Ok((status, _)) = http_get(server.addr, "/", Duration::from_millis(500)) {
            assert_eq!(status, 503, "{}", diagnostics(&server));
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    assert_exit_code(status, 70, &server);
    assert!(
        server_log(&server).contains("flagged unhealthy"),
        "the PHP bootstrap failure must trigger the exit\n{}",
        diagnostics(&server)
    );
}

#[test]
fn a_request_outliving_the_drain_budget_exits_1() {
    let mut server = spawn_with_config(
        FIXTURE,
        1,
        "mode = \"worker\"\n[supervisor]\nprocess_control_timeout_secs = 2\npidfile = \"server.pid\"\n",
    );
    let _connection = open_held_request(&server);
    send_ctrl_break(server.pid());
    let status = server.wait_exit(EXIT);
    assert_exit_code(status, 1, &server);
    let log = server_log(&server);
    assert!(
        log.contains("http drain timed out") || log.contains("shutdown timed out"),
        "the extension's drain error must explain exit 1\n{}",
        diagnostics(&server)
    );
    assert!(
        server.dir.join("server.pid").is_file(),
        "the still-blocked PHP thread requires forced termination, which leaves the pidfile"
    );
}

#[test]
fn a_second_ctrl_break_during_drain_exits_130() {
    let mut server = spawn_with_config(
        FIXTURE,
        1,
        "mode = \"worker\"\n[supervisor]\nprocess_control_timeout_secs = 30\npidfile = \"server.pid\"\n",
    );
    let _connection = open_held_request(&server);
    send_ctrl_break(server.pid());
    assert!(
        wait_log_contains(&server, DRAINING, Duration::from_secs(5)),
        "the first event must reach the registered handler before the second\n{}",
        diagnostics(&server)
    );
    assert!(
        server.try_status().is_none(),
        "the held request keeps the drain active"
    );
    send_ctrl_break(server.pid());
    let status = server.wait_exit(Duration::from_secs(5));
    assert_exit_code(status, 130, &server);
    assert!(
        server.dir.join("server.pid").is_file(),
        "forced termination must leave the pidfile"
    );
}

#[test]
fn a_second_ctrl_break_during_php_join_exits_130() {
    let mut server = spawn_with_config(
        FIXTURE,
        1,
        "mode = \"worker\"\n[supervisor]\nprocess_control_timeout_secs = 6\npidfile = \"server.pid\"\n",
    );
    assert_eq!(
        http_get(server.addr, "/?join=1", REQUEST).unwrap(),
        (200, b"done".to_vec())
    );
    assert!(
        wait_log_contains(&server, "lifecycle-join-blocked", REQUEST),
        "the response must finish before the worker blocks\n{}",
        diagnostics(&server)
    );
    send_ctrl_break(server.pid());
    assert!(
        wait_log_contains(
            &server,
            "drained cleanly; accept loop stopped",
            Duration::from_secs(2)
        ),
        "HTTP must finish draining while PHP remains blocked\n{}",
        diagnostics(&server)
    );
    assert!(
        wait_log_contains(&server, "stopping worker threads", Duration::from_secs(2)),
        "the second event must arrive during the PHP join grace\n{}",
        diagnostics(&server)
    );
    assert!(
        server.try_status().is_none(),
        "the PHP worker is still blocked"
    );
    send_ctrl_break(server.pid());
    let status = server.wait_exit(Duration::from_secs(3));
    assert_exit_code(status, 130, &server);
    assert!(
        server.dir.join("server.pid").is_file(),
        "the control handler must force termination during PHP join and leave the pidfile"
    );
}

#[test]
fn an_occupied_port_fails_before_any_php_worker_starts() {
    let occupied = TcpListener::bind(("127.0.0.1", 0)).expect("reserve occupied port");
    let addr = occupied.local_addr().unwrap();
    // Do not run connect-only readiness against a port held by this test.
    let mut server = spawn_on_addr_unchecked(FIXTURE, 2, "mode = \"worker\"\n", addr);
    let status = server.wait_exit(EXIT);
    assert_exit_code(status, 1, &server);
    let log = server_log(&server);
    assert!(
        log.contains("bind") && log.contains(&addr.to_string()),
        "the occupied address must be reported as a bind failure\n{}",
        diagnostics(&server)
    );
    assert!(
        !log.lines()
            .any(|line| line.contains("worker thread ") && line.contains(" ready")),
        "listener preparation must fail before a worker becomes ready\n{log}"
    );
    assert!(
        !log.contains("booting with mode:") && !log.contains("lifecycle-php-bootstrap"),
        "the bind conflict must be resolved before PHP starts\n{log}"
    );
}

#[test]
fn ctrl_break_drains_a_keepalive_request_and_the_same_port_rebinds() {
    let mut server = spawn_with_config(
        FIXTURE,
        1,
        "mode = \"worker\"\n[supervisor]\nprocess_control_timeout_secs = 6\npidfile = \"server.pid\"\n",
    );
    let addr = server.addr;
    let mut connection = Conn::open(addr, REQUEST).expect("connect keepalive");
    connection
        .send(b"GET /?drain=1 HTTP/1.1\r\nHost: e2e\r\nConnection: keep-alive\r\n\r\n")
        .expect("send keepalive request");
    assert!(
        wait_log_contains(&server, "lifecycle-drain-entered", REQUEST),
        "{}",
        diagnostics(&server)
    );
    send_ctrl_break(server.pid());
    assert!(
        wait_log_contains(&server, DRAINING, Duration::from_secs(2)),
        "the request must still be pending when graceful drain begins\n{}",
        diagnostics(&server)
    );
    std::fs::write(server.dir.join("release-drain"), b"release").expect("release held request");
    let (status, headers) = connection
        .read_head(REQUEST)
        .expect("drained response head");
    assert_eq!(status, 200);
    assert!(
        headers
            .iter()
            .any(|(name, value)| name == "content-length" && value == "8")
    );
    assert_eq!(connection.read_n(8, REQUEST).unwrap(), b"finished");
    assert!(
        connection
            .read_remaining(REQUEST)
            .expect("graceful EOF")
            .is_empty()
    );
    let status = server.wait_exit(EXIT);
    assert_exit_code(status, 0, &server);
    assert!(
        !server.dir.join("server.pid").exists(),
        "a clean drain removes the pidfile"
    );

    // Retain the original client socket while a new server uses the same address.
    let rebound = spawn_on_addr_unchecked(FIXTURE, 1, "mode = \"worker\"\n", addr);
    wait_workers(&rebound, REQUEST, "rebound worker", |ready| {
        ready.len() == 1
    });
    assert_eq!(
        http_get(addr, "/", REQUEST).unwrap(),
        (200, b"ready".to_vec())
    );
    drop(connection);
}

const POOL_CASE: &str = "RAPIRA_E2E_POOL_CASE";

#[test]
fn four_threads_handle_requests_after_their_first_interpreter_recycle() {
    let log = run_isolated_test(
        "lifecycle::pool_probe_child",
        POOL_CASE,
        "quota",
        Duration::from_secs(60),
    );
    for index in 0..4 {
        assert!(
            log.contains(&format!("worker thread {index} recycling")),
            "{log}"
        );
    }
}

#[test]
fn fifty_interpreter_recycles_keep_php_memory_bounded() {
    let log = run_isolated_test(
        "lifecycle::pool_probe_child",
        POOL_CASE,
        "memory",
        Duration::from_secs(60),
    );
    assert!(log.contains("memory samples:"), "{log}");
}

#[test]
fn stopping_a_pool_interrupts_a_backoff_longer_than_the_join_grace() {
    run_isolated_test(
        "lifecycle::pool_probe_child",
        POOL_CASE,
        "backoff",
        Duration::from_secs(40),
    );
}

#[test]
fn pool_probe_child() {
    let Ok(case) = std::env::var(POOL_CASE) else {
        return;
    };
    let _php = tests::php_lock();
    tests::init_log_capture();
    match case.as_str() {
        "quota" => prove_per_thread_recycle(),
        "memory" => prove_memory_bound(),
        "backoff" => prove_interruptible_backoff(),
        _ => panic!("unknown pool case: {case}"),
    }
    for record in tests::captured()
        .iter()
        .filter(|record| record.target == "rapira")
    {
        println!("{}", record.message);
    }
}

fn pool_request(script: &std::path::Path) -> php_sys::Request {
    let mut request = tests::req("/", "unused.php");
    request.script_filename = script.to_path_buf();
    request.document_root = script.parent().unwrap().to_string_lossy().into_owned();
    request
}

fn prove_per_thread_recycle() {
    let script = fixture_path("lifecycle/quota-worker.php");
    let pool = php_sys::Rapira::start_pool(
        php_sys::Mode::Worker(script.clone()),
        4,
        php_sys::PoolHooks {
            max_requests: 1,
            ..Default::default()
        },
    )
    .unwrap();
    let handle = pool.handle();
    let mut first_recycle_handled = BTreeMap::new();
    let deadline = Instant::now() + Duration::from_secs(40);
    loop {
        let responses: Vec<_> = (0..16)
            .map(|_| handle.handle_blocking(pool_request(&script)).unwrap())
            .collect();
        for response in responses {
            let response = tests::drain_resp(response);
            assert_eq!(response.status(), 200);
            assert_eq!(response.body, b"ok");
            assert!(response.ended && !response.truncated);
        }
        let snapshot = pool.scoreboard();
        assert_eq!(snapshot.workers.len(), 4);
        for slot in &snapshot.workers {
            assert_eq!(slot.errors, 0);
            if slot.recycles > 0 {
                first_recycle_handled.entry(slot.id).or_insert(slot.handled);
            }
        }
        if first_recycle_handled.len() == 4
            && snapshot
                .workers
                .iter()
                .all(|slot| slot.recycles >= 2 && slot.handled > first_recycle_handled[&slot.id])
        {
            for slot in snapshot.workers {
                println!(
                    "slot {} handled {} after first-recycle baseline {}, recycles {}",
                    slot.id, slot.handled, first_recycle_handled[&slot.id], slot.recycles
                );
            }
            break;
        }
        assert!(
            Instant::now() < deadline,
            "not every slot served after recycling: {snapshot:?}; baselines {first_recycle_handled:?}"
        );
    }
    drop(handle);
    assert!(pool.shutdown(), "all four PHP threads must join");
}

fn prove_memory_bound() {
    // Local php-src Zend/zend_alloc_sizes.h defines ZEND_MM_CHUNK_SIZE as 2 MiB.
    // This fixture retains 256 KiB for each request and processes at most two requests for each generation when max_requests=1. Its 512 KiB active payload uses one Zend chunk. A second chunk provides space for allocator metadata and temporary values. Therefore, the fixed allowance is 4 MiB above the first sample and does not depend on 50 recycles.
    const ZEND_CHUNK: u64 = 2 * 1024 * 1024;
    const REQUEST_ALLOCATION: u64 = 256 * 1024;
    const GROWTH_ALLOWANCE: u64 =
        (2 * REQUEST_ALLOCATION).div_ceil(ZEND_CHUNK) * ZEND_CHUNK + ZEND_CHUNK;
    let script = fixture_path("lifecycle/memory-worker.php");
    let pool = php_sys::Rapira::start_pool(
        php_sys::Mode::Worker(script.clone()),
        1,
        php_sys::PoolHooks {
            max_requests: 1,
            ..Default::default()
        },
    )
    .unwrap();
    let handle = pool.handle();
    let mut samples = Vec::new();
    let mut completed = false;
    for _ in 0..104 {
        let response = tests::drain_resp(handle.handle_blocking(pool_request(&script)).unwrap());
        assert_eq!(response.status(), 200);
        assert!(response.ended && !response.truncated);
        let values: serde_json::Value = serde_json::from_slice(&response.body).unwrap();
        assert!(
            (1..=2).contains(&values["held"].as_u64().unwrap()),
            "generation quota did not reset: {values}"
        );
        samples.push(values["memory"].as_u64().unwrap());
        let snapshot = pool.scoreboard();
        assert_eq!(snapshot.errors, 0);
        if snapshot.workers[0].recycles >= 50 {
            assert!(snapshot.workers[0].handled >= 100);
            completed = true;
            break;
        }
    }
    assert!(
        completed,
        "fewer than fifty interpreter recycles: {:?}",
        pool.scoreboard()
    );
    let minimum = *samples.iter().min().unwrap();
    let maximum = *samples.iter().max().unwrap();
    let limit = samples[0] + GROWTH_ALLOWANCE;
    println!(
        "memory samples: count={}, minimum={minimum}, maximum={maximum}, limit={limit}, allowance={GROWTH_ALLOWANCE}",
        samples.len()
    );
    assert!(
        maximum <= limit,
        "PHP memory grew across fifty recycles: min={minimum}, max={maximum}, limit={limit}"
    );
    drop(handle);
    assert!(pool.shutdown(), "the memory-probe PHP thread must join");
}

fn prove_interruptible_backoff() {
    let script = fixture_path("lifecycle/fatal-worker.php");
    let pool = php_sys::Rapira::start_pool(
        php_sys::Mode::Worker(script.clone()),
        1,
        php_sys::PoolHooks::default(),
    )
    .unwrap();
    let handle = pool.handle();
    // Four rejected jobs let each interpreter reach its fifth failed start.
    let responses: Vec<_> = (0..64)
        .map(|_| handle.handle_blocking(pool_request(&script)).unwrap())
        .collect();
    let deadline = Instant::now() + Duration::from_secs(25);
    loop {
        let unhealthy_generations = tests::captured()
            .iter()
            .filter(|record| {
                record
                    .message
                    .contains("worker keeps failing to boot; flagged unhealthy")
            })
            .count();
        if unhealthy_generations >= 7 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "did not reach the 6.4-second backoff: {:?}",
            pool.scoreboard()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    // The seventh immediate failure waits 6.4 seconds. Let interpreter teardown finish before stopping to test an active retry delay longer than the five-second grace period.
    std::thread::sleep(Duration::from_millis(100));
    let started = Instant::now();
    assert!(
        pool.shutdown(),
        "stop must interrupt the active crash backoff"
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the stop consumed its join grace"
    );
    drop(handle);
    drop(responses);
}

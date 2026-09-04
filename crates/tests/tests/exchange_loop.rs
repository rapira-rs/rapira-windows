use php_sys::{Frame, Mode, Rapira};
use std::io::Cursor;
use tests::{captured, drain, drain_resp, fixture, init_log_capture, php_lock, req};

fn verbs_probe(query: &str) -> anyhow::Result<(u16, String)> {
    let _guard = php_lock();
    let r = Rapira::start(Mode::Dispatcher(fixture("dispatcher/verbs-worker.php")))?;
    let h = r.handle();
    let out = drain(h.handle_blocking(req(query, "dispatcher/verbs-worker.php"))?);
    drop(h);
    r.shutdown();
    Ok(out)
}

/// Processes two sequential units in the echo loop. Dropping the handle and pool must cause the waiting `receive()` to throw `ClosedException`.
#[test]
fn exchange_serves_sequential_requests() -> anyhow::Result<()> {
    let _guard = php_lock();
    init_log_capture();
    captured().clear();

    let r = Rapira::start(Mode::Dispatcher(fixture("dispatcher/echo-loop-worker.php")))?;
    let h = r.handle();

    let resp = drain_resp(h.handle_blocking(req("/first", "dispatcher/echo-loop-worker.php"))?);
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.header("x-rapira-target").as_deref(), Some("/first"));
    assert_eq!(
        resp.body_string(),
        "method=GET body=",
        "empty request body echoes empty"
    );

    let mut rq2 = req("/second", "dispatcher/echo-loop-worker.php");
    rq2.body = php_sys::types::Body::Raw(Box::new(Cursor::new(b"two".to_vec())));
    rq2.content_length = 3;
    let resp = drain_resp(h.handle_blocking(rq2)?);
    assert_eq!(resp.header("x-rapira-target").as_deref(), Some("/second"));
    assert_eq!(resp.body_string(), "method=GET body=two");

    drop(h);
    r.shutdown();

    let drained = captured()
        .iter()
        .filter(|c| c.target == "app" && c.message == "drained")
        .count();
    assert_eq!(
        drained, 1,
        "ClosedException must reach the fixture exactly once"
    );
    Ok(())
}

/// On an empty open channel, `tryReceive()`, `receive(0)`, and `receive(50ms)` must report null or a timeout. They must not report `Closed`.
#[test]
fn recv_probes_on_an_empty_channel() -> anyhow::Result<()> {
    let _guard = php_lock();
    init_log_capture();
    captured().clear();

    let r = Rapira::start(Mode::Dispatcher(fixture(
        "dispatcher/recv-probes-worker.php",
    )))?;

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if captured()
            .iter()
            .any(|c| c.target == "app" && c.message == "recv-probes")
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "recv-probes record never appeared"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    r.shutdown();

    let contexts: Vec<String> = captured()
        .iter()
        .filter(|c| c.target == "app" && c.message == "recv-probes")
        .map(|c| c.context.clone())
        .collect();
    assert_eq!(contexts.len(), 1, "one probe record (got {contexts:?})");
    for fragment in [
        r#""try":"null""#,
        r#""zero":"timeout""#,
        r#""short":"timeout""#,
    ] {
        assert!(
            contexts[0].contains(fragment),
            "missing {fragment} in {:?}",
            contexts[0]
        );
    }
    Ok(())
}

/// A `writeBody()` with no prior `writeHead()` commits an implicit 200.
#[test]
fn implicit_200_on_first_write_body() -> anyhow::Result<()> {
    let (status, body) = verbs_probe("/")?;
    assert_eq!((status, body.as_str()), (200, "state=false"));
    Ok(())
}

/// A second finalization operation after sealing throws `AlreadyFinalizedError`. The operation does not change the sealed response.
#[test]
fn double_finalize_throws_already_finalized() -> anyhow::Result<()> {
    let _guard = php_lock();
    init_log_capture();
    captured().clear();

    let r = Rapira::start(Mode::Dispatcher(fixture("dispatcher/verbs-worker.php")))?;
    let h = r.handle();

    let (status, body) = drain(h.handle_blocking(req(
        "/?probe=double-finalize",
        "dispatcher/verbs-worker.php",
    ))?);
    assert_eq!((status, body.as_str()), (200, "first"));

    drop(h);
    r.shutdown();

    let records: Vec<String> = captured()
        .iter()
        .filter(|c| c.target == "app" && c.message == "double-finalize")
        .map(|c| c.context.clone())
        .collect();
    assert_eq!(records.len(), 1, "one throw record (got {records:?})");
    assert!(
        records[0].contains(r#""class":"Rapira\\Exception\\AlreadyFinalizedError""#),
        "wrong exception class: {:?}",
        records[0]
    );
    Ok(())
}

/// A second `writeHead()` after the final head throws `HeadAlreadyWrittenError`. The response retains the first head.
#[test]
fn double_head_throws_head_already_written() -> anyhow::Result<()> {
    let (status, body) = verbs_probe("/?probe=double-head")?;
    assert_eq!(status, 201, "the first head must stand");
    assert_eq!(
        body,
        "double-head:Rapira\\Http\\Exception\\HeadAlreadyWrittenError"
    );
    Ok(())
}

/// Out-of-range status values and invalid header names, values, and formats raise `\ValueError` before the host receives data.
#[test]
fn status_range_and_header_shape_value_errors() -> anyhow::Result<()> {
    let (status, body) = verbs_probe("/?probe=value-errors")?;
    assert_eq!(
        (status, body.as_str()),
        (200, "range:99;range:600;name;value;shape;intkey;item")
    );
    Ok(())
}

/// An empty chunk without `eos` must not commit the implicit 200 response or prevent a later `writeHead()`.
#[test]
fn empty_non_eos_chunk_does_nothing() -> anyhow::Result<()> {
    let (status, body) = verbs_probe("/?probe=empty-chunk")?;
    assert_eq!((status, body.as_str()), (404, "body"));
    Ok(())
}

/// Checks operation limits. `tryReceive()` with an active unit throws the single-flight `\Error`. A timeout below -1 throws `\ValueError`. `writeHead()` after `eos` throws `HeadAlreadyWrittenError`.
#[test]
fn verb_edges_throw_their_documented_classes() -> anyhow::Result<()> {
    let _guard = php_lock();
    init_log_capture();
    captured().clear();

    let r = Rapira::start(Mode::Dispatcher(fixture("dispatcher/verbs-worker.php")))?;
    let h = r.handle();
    let (status, body) =
        drain(h.handle_blocking(req("/?probe=verb-edges", "dispatcher/verbs-worker.php"))?);
    assert_eq!((status, body.as_str()), (200, "try-busy;neg-timeout"));
    drop(h);
    r.shutdown();

    let records: Vec<String> = captured()
        .iter()
        .filter(|c| c.target == "app" && c.message == "head-after-eos")
        .map(|c| c.context.clone())
        .collect();
    assert_eq!(records.len(), 1, "one throw record (got {records:?})");
    assert!(
        records[0].contains(r#""class":"Rapira\\Http\\Exception\\HeadAlreadyWrittenError""#),
        "wrong exception class: {:?}",
        records[0]
    );
    Ok(())
}

/// Checks successful polling. `tryReceive()` returns one unit. `receive(1s)` returns one unit after the fixture changes modes.
#[test]
fn try_and_timed_receive_serve_units() -> anyhow::Result<()> {
    let _guard = php_lock();
    let r = Rapira::start(Mode::Dispatcher(fixture("dispatcher/poll-worker.php")))?;
    let h = r.handle();

    let (status, body) = drain(h.handle_blocking(req("/one", "dispatcher/poll-worker.php"))?);
    assert_eq!((status, body.as_str()), (200, "served-by=try target=/one"));

    let (status, body) =
        drain(h.handle_blocking(req("/two?mode=timed", "dispatcher/poll-worker.php"))?);
    assert_eq!(
        (status, body.as_str()),
        (200, "served-by=try target=/two?mode=timed")
    );

    let (status, body) = drain(h.handle_blocking(req("/three", "dispatcher/poll-worker.php"))?);
    assert_eq!(
        (status, body.as_str()),
        (200, "served-by=timed target=/three")
    );

    drop(h);
    r.shutdown();
    Ok(())
}

/// A 1xx head other than 101 is sent immediately before the final head. The unit remains open for the final head.
#[test]
fn interim_head_is_emitted_before_the_final_head() -> anyhow::Result<()> {
    let _guard = php_lock();
    let r = Rapira::start(Mode::Dispatcher(fixture("dispatcher/verbs-worker.php")))?;
    let h = r.handle();
    let resp =
        drain_resp(h.handle_blocking(req("/?probe=interim", "dispatcher/verbs-worker.php"))?);
    drop(h);
    r.shutdown();

    assert_eq!(resp.interim.len(), 1, "the 103 must reach the stream");
    assert_eq!(resp.interim[0].status, 103);
    assert!(
        resp.interim[0].headers.iter().any(|(k, _)| k == "link"),
        "interim fields travel with it"
    );
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.body_string(), "after-interim finalized=false");
    Ok(())
}

/// A 101 response is final, so it prevents later `writeHead()` calls and has no body.
#[test]
fn writehead_101_commits_as_final() -> anyhow::Result<()> {
    let _guard = php_lock();
    init_log_capture();
    captured().clear();
    let r = Rapira::start(Mode::Dispatcher(fixture("dispatcher/verbs-worker.php")))?;
    let h = r.handle();
    let resp =
        drain_resp(h.handle_blocking(req("/?probe=upgrade", "dispatcher/verbs-worker.php"))?);
    drop(h);
    r.shutdown();

    assert_eq!(resp.status(), 101);
    assert!(resp.bodiless, "the Head frame marks a 1xx bodiless");
    assert!(resp.body.is_empty(), "1xx carries no body");
    assert!(
        captured()
            .iter()
            .any(|c| c.target == "app" && c.message == "101-locked"),
        "the second writeHead must throw HeadAlreadyWrittenError"
    );
    Ok(())
}

/// Buffered chunks concatenate, and the first chunk commits the implicit 200 response. A later `writeHead()` must throw and must not change the status.
#[test]
fn chunked_body_buffers_and_locks_the_head() -> anyhow::Result<()> {
    let (status, body) = verbs_probe("/?probe=chunks")?;
    assert_eq!((status, body.as_str()), (200, "one-mid=false"));

    let (status, body) = verbs_probe("/?probe=head-after-chunk")?;
    assert_eq!((status, body.as_str()), (200, "partial|locked"));
    Ok(())
}

/// Multi-value lists produce one field line for each value. The function dereferences PHP references at both nesting levels.
#[test]
fn multi_value_and_reference_headers_flatten() -> anyhow::Result<()> {
    let _guard = php_lock();
    let r = Rapira::start(Mode::Dispatcher(fixture("dispatcher/verbs-worker.php")))?;
    let h = r.handle();

    let resp = drain_resp(h.handle_blocking(req("/?probe=multi", "dispatcher/verbs-worker.php"))?);
    let head = resp.head.as_ref().expect("head committed");
    assert_eq!(head.status, 200);
    let multi: Vec<String> = head
        .headers
        .iter()
        .filter(|(k, _)| k == "x-multi")
        .map(|(_, v)| String::from_utf8_lossy(v).into_owned())
        .collect();
    assert_eq!(multi, ["a", "b"], "one field line per list value, in order");
    assert_eq!(resp.header("x-ref").as_deref(), Some("r1"));
    assert_eq!(resp.header("x-vref").as_deref(), Some("c1"));

    drop(h);
    r.shutdown();
    Ok(())
}

/// `receive()` throws the single-flight `\Error` while a unit is not finalized. This prevents the worker from waiting for itself.
#[test]
fn receive_while_unfinalized_throws() -> anyhow::Result<()> {
    let (status, body) = verbs_probe("/?probe=busy")?;
    assert_eq!(status, 200);
    assert!(
        body.contains("busy:receive() while a Rapira\\Http\\Exchange is unfinalized"),
        "single-flight error must surface: {body:?}"
    );
    Ok(())
}

/// Dropping an `Exchange` without finalization fails only that unit. The host returns 500, and the worker processes the next unit.
#[test]
fn abandoned_exchange_fails_that_unit_only() -> anyhow::Result<()> {
    let _guard = php_lock();
    let r = Rapira::start(Mode::Dispatcher(fixture("dispatcher/verbs-worker.php")))?;
    let h = r.handle();

    let resp =
        drain_resp(h.handle_blocking(req("/?probe=abandon", "dispatcher/verbs-worker.php"))?);
    assert_eq!(
        (
            resp.status(),
            resp.body.as_slice(),
            resp.truncated,
            resp.ended
        ),
        (500, &b""[..], false, true),
        "an abandoned unit is failed by the host with a complete 500"
    );

    let (status, body) = drain(h.handle_blocking(req("/", "dispatcher/verbs-worker.php"))?);
    assert_eq!(
        (status, body.as_str()),
        (200, "state=false"),
        "the worker must keep serving after an abandoned unit"
    );

    drop(h);
    r.shutdown();
    Ok(())
}

/// A unit that ends with its cycle is a worker failure. The channel closes without data, and the host does not create a 500 response.
#[test]
fn bailout_with_unit_out_dies_unsent() -> anyhow::Result<()> {
    let _guard = php_lock();
    let r = Rapira::start(Mode::Dispatcher(fixture("dispatcher/verbs-worker.php")))?;
    let h = r.handle();

    let resp = drain_resp(
        h.handle_blocking(req("/?probe=bail-with-unit", "dispatcher/verbs-worker.php"))?,
    );
    assert!(
        resp.head.is_none() && !resp.ended,
        "a cycle-death loss must stay unsent (got status {})",
        resp.status()
    );

    let (status, body) = drain(h.handle_blocking(req("/", "dispatcher/verbs-worker.php"))?);
    assert_eq!(
        (status, body.as_str()),
        (200, "state=false"),
        "the recycled worker must keep serving"
    );

    drop(h);
    r.shutdown();
    Ok(())
}

/// An explicit `Work::__destruct()` call on an active unit does nothing.
#[test]
fn explicit_destruct_call_is_a_noop() -> anyhow::Result<()> {
    let (status, body) = verbs_probe("/?probe=destruct-explicit")?;
    assert_eq!((status, body.as_str()), (200, "explicit-destruct-ok"));
    Ok(())
}

/// Dropping an `Exchange` after sending the head cannot produce a 500 response. The host ends the stream as truncated so the client detects the incomplete response.
#[test]
fn abandoned_mid_stream_exchange_truncates() -> anyhow::Result<()> {
    let _guard = php_lock();
    let r = Rapira::start(Mode::Dispatcher(fixture("dispatcher/verbs-worker.php")))?;
    let h = r.handle();

    let resp =
        drain_resp(h.handle_blocking(req("/?probe=abandon-mid", "dispatcher/verbs-worker.php"))?);
    assert_eq!(resp.status(), 200, "the committed head stands");
    assert_eq!(resp.body, b"partial");
    assert!(resp.truncated, "the cut must be visible to the client");

    let (status, body) = drain(h.handle_blocking(req("/", "dispatcher/verbs-worker.php"))?);
    assert_eq!((status, body.as_str()), (200, "state=false"));

    drop(h);
    r.shutdown();
    Ok(())
}

/// A dropped unit with a multipart body parsed by the host must unlink its spool file when the exchange ends.
#[test]
fn abandoned_multipart_unit_unlinks_its_spool() -> anyhow::Result<()> {
    let _guard = php_lock();
    let r = Rapira::start(Mode::Dispatcher(fixture("dispatcher/verbs-worker.php")))?;
    let h = r.handle();

    let spool = std::env::temp_dir().join(format!("rapira-test-abandon-{}", std::process::id()));
    std::fs::write(&spool, b"PAYLOAD")?;
    let mut rq = req("/?probe=abandon", "dispatcher/verbs-worker.php");
    rq.body = php_sys::types::Body::Multipart(php_sys::types::MultipartBody {
        fields: vec![],
        files: vec![php_sys::types::UploadedFile {
            name: b"f".to_vec(),
            client_filename: b"a.bin".to_vec(),
            client_media_type: None,
            headers: vec![],
            file: php_sys::types::SpooledFile {
                path: spool.clone(),
            },
            size: 7,
        }],
    });
    let (status, body) = drain(h.handle_blocking(rq)?);
    assert_eq!(
        (status, body.as_str()),
        (500, ""),
        "the host fails the unit"
    );
    assert!(
        !spool.exists(),
        "dropping the exchange must unlink the spooled file"
    );

    drop(h);
    r.shutdown();
    Ok(())
}

/// `exit()` after processing a request must produce `Cycle::Recycle`. The script runs again and does not report a startup failure.
#[test]
fn exit_after_serving_recycles_the_worker() -> anyhow::Result<()> {
    let _guard = php_lock();
    let r = Rapira::start(Mode::Dispatcher(fixture("dispatcher/verbs-worker.php")))?;
    let h = r.handle();

    let (status, body) =
        drain(h.handle_blocking(req("/?probe=exit", "dispatcher/verbs-worker.php"))?);
    assert_eq!((status, body.as_str()), (200, "bye"));

    let (status, body) = drain(h.handle_blocking(req("/", "dispatcher/verbs-worker.php"))?);
    assert_eq!(
        (status, body.as_str()),
        (200, "state=false"),
        "the worker must re-run the script after exit()"
    );

    drop(h);
    r.shutdown();
    Ok(())
}

/// A `HEAD` or 204 response has no body. Sealing discards chunks and retains the head.
/// https://www.rfc-editor.org/rfc/rfc9112#section-6.3
#[test]
fn head_and_204_drop_the_body() -> anyhow::Result<()> {
    let _guard = php_lock();
    let r = Rapira::start(Mode::Dispatcher(fixture("dispatcher/verbs-worker.php")))?;
    let h = r.handle();

    let mut head_rq = req("/", "dispatcher/verbs-worker.php");
    head_rq.method = "HEAD".into();
    let (status, body) = drain(h.handle_blocking(head_rq)?);
    assert_eq!(
        (status, body.as_str()),
        (200, ""),
        "HEAD keeps the GET head but drops the body"
    );

    let (status, body) =
        drain(h.handle_blocking(req("/?probe=head204", "dispatcher/verbs-worker.php"))?);
    assert_eq!((status, body.as_str()), (204, ""));

    drop(h);
    r.shutdown();
    Ok(())
}

/// Checks the `Rapira\Http\Request` field mapping: headers for each line, exact `$target` bytes, `$authority`, constructed `$uri`, address objects, null `$tls`, and the fallback receive timestamp.
#[test]
fn request_fields_reach_php() -> anyhow::Result<()> {
    let _guard = php_lock();
    let r = Rapira::start(Mode::Dispatcher(fixture("dispatcher/request-worker.php")))?;
    let h = r.handle();

    let mut rq = req("/path?x=1", "dispatcher/request-worker.php");
    rq.headers = vec![
        ("x-probe".into(), b"alpha".to_vec()),
        ("X-Case".into(), b"one".to_vec()),
        ("x-probe".into(), b"beta".to_vec()),
        ("x-case".into(), b"two".to_vec()),
        ("123".into(), b"numeric".to_vec()),
        ("a".into(), b"solo".to_vec()),
        ("-".into(), b"dash".to_vec()),
        ("-1".into(), b"neg".to_vec()),
    ];
    rq.authority = Some(b"example.test".to_vec());
    rq.target = Some(b"/path%2Fa?x=1\xe9".to_vec());
    rq.body = php_sys::types::Body::Raw(Box::new(Cursor::new(b"hello".to_vec())));
    rq.content_length = 5;
    let (status, body) = drain(h.handle_blocking(rq)?);
    assert_eq!(status, 200);
    let target_hex = format!(
        "target-hex={}",
        b"/path%2Fa?x=1\xe9"
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    );
    assert!(
        body.contains(&target_hex),
        "missing {target_hex:?} in {body:?}"
    );
    for line in [
        "method=GET",
        "uri=http://example.test/path?x=1",
        "authority='example.test'",
        "protocol=HTTP/1.1",
        "x-probe=alpha|beta",
        "x-case-keys=X-Case|x-case",
        "h123=numeric",
        "h-single=solo",
        "h-dash=dash",
        "h-neg=neg",
        "memo-same=true",
        "body=hello",
        "remote=Rapira\\InetAddress",
        "remote-detail=127.0.0.1:8080",
        "server=Rapira\\InetAddress",
        "server-detail=127.0.0.1:8080",
        "tls=NULL",
        "received-at-positive=true",
    ] {
        assert!(body.contains(line), "missing {line:?} in {body:?}");
    }

    let (status, body) = drain(h.handle_blocking(req("/again", "dispatcher/request-worker.php"))?);
    assert_eq!(status, 200, "body: {body:?}");
    assert!(
        body.contains("memo-same=true"),
        "fresh memo on the new unit"
    );

    drop(h);
    r.shutdown();
    Ok(())
}

/// Preserves request properties from the producer: exact `receivedAt`, the API protocol value, and the server socket URI when the request has no authority.
#[test]
fn plugin_stamped_fields_pass_through() -> anyhow::Result<()> {
    let _guard = php_lock();
    let r = Rapira::start(Mode::Dispatcher(fixture("dispatcher/request-worker.php")))?;
    let h = r.handle();

    let mut rq = req("/p", "dispatcher/request-worker.php");
    rq.protocol = "HTTP/2.0".into();
    rq.received_at = Some(123.5);
    let (status, body) = drain(h.handle_blocking(rq)?);
    assert_eq!(status, 200);
    for line in [
        "protocol=HTTP/2",
        "received-at=123.5",
        "authority=NULL",
        "uri=http://127.0.0.1:8080/p",
    ] {
        assert!(body.contains(line), "missing {line:?} in {body:?}");
    }

    drop(h);
    r.shutdown();
    Ok(())
}

/// Checks the remaining `$uri` construction cases: the HTTPS scheme and an asterisk-form target that uses the authority root.
#[test]
fn uri_synthesis_covers_https_and_asterisk_form() -> anyhow::Result<()> {
    let _guard = php_lock();
    let r = Rapira::start(Mode::Dispatcher(fixture("dispatcher/request-worker.php")))?;
    let h = r.handle();

    let mut rq = req("/secure", "dispatcher/request-worker.php");
    rq.https = true;
    rq.protocol = "HTTP/3.0".into();
    let (status, body) = drain(h.handle_blocking(rq)?);
    assert_eq!(status, 200);
    for line in ["uri=https://127.0.0.1:8080/secure", "protocol=HTTP/3"] {
        assert!(body.contains(line), "missing {line:?} in {body:?}");
    }

    let mut rq = req("*", "dispatcher/request-worker.php");
    rq.method = "OPTIONS".into();
    let (status, body) = drain(h.handle_blocking(rq)?);
    assert_eq!(status, 200);
    for line in [
        "method=OPTIONS",
        "uri=http://127.0.0.1:8080/",
        "target-hex=2a",
    ] {
        assert!(body.contains(line), "missing {line:?} in {body:?}");
    }

    drop(h);
    r.shutdown();
    Ok(())
}

/// Checks TLS values with a complete object that contains a client certificate and with each nullable value.
#[test]
fn tls_view_reaches_php() -> anyhow::Result<()> {
    let _guard = php_lock();
    let r = Rapira::start(Mode::Dispatcher(fixture("dispatcher/request-worker.php")))?;
    let h = r.handle();

    let mut rq = req("/", "dispatcher/request-worker.php");
    rq.tls = Some(php_sys::types::TlsView {
        version: "TLSv1.3".into(),
        cipher: "TLS_AES_256_GCM_SHA384".into(),
        alpn: Some("h2".into()),
        server_name: Some("sni.example".into()),
        cert: Some(php_sys::types::ClientCertView {
            serial: "0AB1".into(),
            organization: None,
            fingerprint: "abcd".into(),
        }),
    });
    let (status, body) = drain(h.handle_blocking(rq)?);
    assert_eq!(status, 200);
    assert!(
        body.contains("tls=TLSv1.3|TLS_AES_256_GCM_SHA384|'h2'|'sni.example'|'0AB1'|NULL|'abcd'"),
        "unexpected tls line in {body:?}"
    );

    let mut rq = req("/", "dispatcher/request-worker.php");
    rq.tls = Some(php_sys::types::TlsView {
        version: "TLSv1.2".into(),
        cipher: "X".into(),
        alpn: None,
        server_name: None,
        cert: None,
    });
    let (status, body) = drain(h.handle_blocking(rq)?);
    assert_eq!(status, 200);
    assert!(
        body.contains("tls=TLSv1.2|X|NULL|NULL|NULL|NULL|NULL"),
        "unexpected tls line in {body:?}"
    );

    drop(h);
    r.shutdown();
    Ok(())
}

/// PHP receives host-parsed multipart data as an object graph. seal() unlinks the spool file before sending the response frame.
#[test]
fn multipart_body_reaches_php_and_spools_die_at_seal() -> anyhow::Result<()> {
    let _guard = php_lock();
    let r = Rapira::start(Mode::Dispatcher(fixture("dispatcher/multipart-worker.php")))?;
    let h = r.handle();

    let spool = std::env::temp_dir().join(format!("rapira-test-mp-{}", std::process::id()));
    std::fs::write(&spool, b"PAYLOAD")?;

    let mut rq = req("/", "dispatcher/multipart-worker.php");
    rq.body = php_sys::types::Body::Multipart(php_sys::types::MultipartBody {
        fields: vec![php_sys::types::FormField {
            name: b"note".to_vec(),
            value: b"hello".to_vec(),
            headers: vec![(
                "content-disposition".into(),
                b"form-data; name=\"note\"".to_vec(),
            )],
        }],
        files: vec![php_sys::types::UploadedFile {
            name: b"f".to_vec(),
            client_filename: b"a.bin".to_vec(),
            client_media_type: Some(b"application/octet-stream".to_vec()),
            headers: vec![(
                "content-disposition".into(),
                b"form-data; name=\"f\"; filename=\"a.bin\"".to_vec(),
            )],
            file: php_sys::types::SpooledFile {
                path: spool.clone(),
            },
            size: 7,
        }],
    });
    let (status, body) = drain(h.handle_blocking(rq)?);
    assert_eq!(status, 200, "body: {body:?}");
    for line in [
        "class=Rapira\\Http\\Multipart",
        "counts=1/1",
        "field0=note=hello",
        "field0-cd=true",
        "file0=f:a.bin:7:PAYLOAD",
        "file0-type='application/octet-stream'",
    ] {
        assert!(body.contains(line), "missing {line:?} in {body:?}");
    }
    assert!(
        !spool.exists(),
        "seal() must unlink the spool before the frame goes out"
    );

    drop(h);
    r.shutdown();
    Ok(())
}

/// Checks two fields and two files. The graph must associate each part with its headers, spool path, and size by index.
#[test]
fn multipart_parts_stay_index_aligned() -> anyhow::Result<()> {
    let _guard = php_lock();
    let r = Rapira::start(Mode::Dispatcher(fixture("dispatcher/multipart-worker.php")))?;
    let h = r.handle();

    let pid = std::process::id();
    let spool_a = std::env::temp_dir().join(format!("rapira-test-mpa-{pid}"));
    let spool_b = std::env::temp_dir().join(format!("rapira-test-mpb-{pid}"));
    std::fs::write(&spool_a, b"AAA")?;
    std::fs::write(&spool_b, b"BBBBB")?;

    let file = |name: &[u8], filename: &[u8], path: &std::path::Path, size: u64| {
        php_sys::types::UploadedFile {
            name: name.to_vec(),
            client_filename: filename.to_vec(),
            client_media_type: None,
            headers: vec![],
            file: php_sys::types::SpooledFile {
                path: path.to_path_buf(),
            },
            size,
        }
    };
    let field = |name: &[u8], value: &[u8]| php_sys::types::FormField {
        name: name.to_vec(),
        value: value.to_vec(),
        headers: vec![("content-disposition".into(), b"form-data".to_vec())],
    };
    let mut rq = req("/", "dispatcher/multipart-worker.php");
    rq.body = php_sys::types::Body::Multipart(php_sys::types::MultipartBody {
        fields: vec![field(b"one", b"1"), field(b"two", b"22")],
        files: vec![
            file(b"fa", b"a.bin", &spool_a, 3),
            file(b"fb", b"b.bin", &spool_b, 5),
        ],
    });
    let (status, body) = drain(h.handle_blocking(rq)?);
    assert_eq!(status, 200, "body: {body:?}");
    for line in [
        "counts=2/2",
        "field0=one=1",
        "field1=two=22",
        "file0=fa:a.bin:3:AAA",
        "file1=fb:b.bin:5:BBBBB",
    ] {
        assert!(body.contains(line), "missing {line:?} in {body:?}");
    }
    assert!(!spool_a.exists() && !spool_b.exists(), "seal unlinks both");

    drop(h);
    r.shutdown();
    Ok(())
}

/// During request processing, `getInfo()` counts the current unit as active and does not count it as pending.
#[test]
fn get_info_counts_the_outstanding_unit() -> anyhow::Result<()> {
    let (status, body) = verbs_probe("/?probe=info")?;
    assert_eq!((status, body.as_str()), (200, "pending=0 active=1"));
    Ok(())
}

// Checks streaming behavior in stream-worker.php after the buffered one-shot response.

fn stream_probe(
    query: &str,
) -> anyhow::Result<(
    Rapira,
    php_sys::RapiraHandle,
    tokio::sync::mpsc::Receiver<Frame>,
)> {
    let r = Rapira::start(Mode::Dispatcher(fixture("dispatcher/stream-worker.php")))?;
    let h = r.handle();
    let rx = h.handle_blocking(req(query, "dispatcher/stream-worker.php"))?;
    Ok((r, h, rx))
}

/// Polls the application log for `message` until a timeout and returns its JSON context.
fn wait_app_record(message: &str) -> String {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if let Some(ctx) = captured()
            .iter()
            .find(|c| c.target == "app" && c.message == message)
            .map(|c| c.context.clone())
        {
            return ctx;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no {message:?} app record within 10s"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// `flush()` sends the head at least 300 ms before the body.
#[test]
fn flush_puts_the_head_on_the_wire_before_eos() -> anyhow::Result<()> {
    let _guard = php_lock();
    let (r, h, mut rx) = stream_probe("/?probe=flush-park")?;

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let first = loop {
        match rx.try_recv() {
            Ok(frame) => break frame,
            Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {
                assert!(
                    std::time::Instant::now() < deadline,
                    "flush never reached the stream"
                );
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                panic!("worker died before flushing")
            }
        }
    };
    let Frame::Head {
        head,
        content_length,
        ..
    } = first
    else {
        panic!("the first frame must be the flushed head");
    };
    assert_eq!(head.status, 200);
    assert_eq!(content_length, None, "flush costs the computed length");
    assert!(
        matches!(
            rx.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        ),
        "no body before the worker wakes"
    );

    let mut body = Vec::new();
    let mut ended = false;
    while let Some(frame) = rx.blocking_recv() {
        match frame {
            Frame::Chunk(b) => body.extend_from_slice(&b),
            Frame::End { truncated, .. } => {
                assert!(!truncated);
                ended = true;
                break;
            }
            _ => {}
        }
    }
    assert!(ended, "the stream must end cleanly");
    assert_eq!(body, b"after");

    drop(h);
    r.shutdown();
    Ok(())
}

/// `writeBody(eos: false)` chunks are sent in order, and the head contains no computed length.
#[test]
fn streamed_chunks_arrive_in_order() -> anyhow::Result<()> {
    let _guard = php_lock();
    let (r, h, rx) = stream_probe("/?probe=chunks")?;
    let resp = drain_resp(rx);
    drop(h);
    r.shutdown();

    assert_eq!(resp.status(), 200);
    assert_eq!(resp.content_length, None);
    assert_eq!(resp.body_string(), "one,two,three");
    assert!(resp.ended && !resp.truncated);
    Ok(())
}

/// When the body exceeds Content-Length, the server sends the bytes that fit and completes the declared response. The write then throws.
#[test]
fn content_length_exceeded_sends_the_fitting_prefix() -> anyhow::Result<()> {
    let _guard = php_lock();
    init_log_capture();
    captured().clear();
    let (r, h, rx) = stream_probe("/?probe=cl-exceeded")?;
    let resp = drain_resp(rx);

    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.content_length,
        Some(5),
        "the declared length is honoured"
    );
    assert_eq!(resp.body_string(), "01234", "the surplus is not sent");
    assert!(
        resp.ended && !resp.truncated,
        "complete per its declaration; keepalive survives"
    );
    let ctx = wait_app_record("cl-exceeded");
    assert!(
        ctx.contains(r#"ContentLengthExceededError"#),
        "wrong class in {ctx}"
    );

    drop(h);
    r.shutdown();
    Ok(())
}

/// Dropping the receiver during a unit makes the next write throw WorkDiscardedException. The unit reports cancelled and finalized, and the worker continues to process requests.
#[test]
fn dropped_client_discards_the_unit() -> anyhow::Result<()> {
    let _guard = php_lock();
    init_log_capture();
    captured().clear();
    let r = Rapira::start(Mode::Dispatcher(fixture("dispatcher/stream-worker.php")))?;
    let h = r.handle();

    let rx = h.handle_blocking(req("/?probe=discard", "dispatcher/stream-worker.php"))?;
    wait_app_record("discard-held");
    drop(rx);

    let ctx = wait_app_record("discard");
    assert!(
        ctx.contains("WorkDiscardedException"),
        "wrong class in {ctx}"
    );
    assert!(ctx.contains(r#""cancelled":true"#), "isCancelled in {ctx}");
    assert!(ctx.contains(r#""finalized":true"#), "isFinalized in {ctx}");

    let resp =
        drain_resp(h.handle_blocking(req("/?probe=chunks", "dispatcher/stream-worker.php"))?);
    assert_eq!(resp.body_string(), "one,two,three");

    drop(h);
    r.shutdown();
    Ok(())
}

/// A declared Content-Length is in the `Head` frame. PHP does not receive an error when the body is shorter.
#[test]
fn declared_content_length_rides_the_head_frame() -> anyhow::Result<()> {
    let _guard = php_lock();
    let (r, h, rx) = stream_probe("/?probe=declared-cl")?;
    let resp = drain_resp(rx);
    drop(h);
    r.shutdown();

    assert_eq!(resp.content_length, Some(10));
    assert_eq!(resp.body_string(), "abc");
    assert!(resp.ended && !resp.truncated);
    Ok(())
}

// Checks host streaming of files through sendFile in stream-worker.php.

/// Writes a temporary payload and sets the sendFile root to the temporary directory.
fn sendfile_setup(name: &str) -> std::path::PathBuf {
    php_sys::set_sendfile_root(std::env::temp_dir());
    let path = std::env::temp_dir().join(format!("rapira-test-{name}-{}", std::process::id()));
    std::fs::write(&path, b"abcdefghijklmnopqrstuvwxyz").expect("write payload");
    path
}

fn with_path_header(query: &str, path: &std::path::Path) -> php_sys::Request {
    let mut rq = req(query, "dispatcher/stream-worker.php");
    rq.headers.push((
        "x-path".into(),
        path.to_string_lossy().into_owned().into_bytes(),
    ));
    rq
}

/// Checks one sendFile call. The head contains the actual Content-Length, and the `File` frame contains the file bytes.
#[test]
fn sendfile_one_shot_carries_the_file_length() -> anyhow::Result<()> {
    let _guard = php_lock();
    let path = sendfile_setup("sendfile");
    let r = Rapira::start(Mode::Dispatcher(fixture("dispatcher/stream-worker.php")))?;
    let h = r.handle();
    let resp = drain_resp(h.handle_blocking(with_path_header("/?probe=sendfile", &path))?);
    drop(h);
    r.shutdown();
    std::fs::remove_file(&path).ok();

    assert_eq!(resp.status(), 200);
    assert_eq!(resp.content_length, Some(26));
    assert_eq!(resp.body, b"abcdefghijklmnopqrstuvwxyz");
    assert!(resp.ended && !resp.truncated);
    Ok(())
}

/// The handler sets status 206 and Content-Range for a range response. It passes the range as an offset and length because sendFile does not apply HTTP range rules.
#[test]
fn sendfile_slice_serves_the_named_bytes() -> anyhow::Result<()> {
    let _guard = php_lock();
    let path = sendfile_setup("slice");
    let r = Rapira::start(Mode::Dispatcher(fixture("dispatcher/stream-worker.php")))?;
    let h = r.handle();
    let resp = drain_resp(h.handle_blocking(with_path_header("/?probe=sendfile-slice", &path))?);
    drop(h);
    r.shutdown();
    std::fs::remove_file(&path).ok();

    assert_eq!(resp.status(), 206);
    assert_eq!(resp.content_length, Some(3));
    assert_eq!(resp.body, b"cde");
    Ok(())
}

/// FileNotSendableException is raised before anything is written, so the handler can still answer 404.
#[test]
fn sendfile_missing_file_still_answers_404() -> anyhow::Result<()> {
    let _guard = php_lock();
    php_sys::set_sendfile_root(std::env::temp_dir());
    let r = Rapira::start(Mode::Dispatcher(fixture("dispatcher/stream-worker.php")))?;
    let h = r.handle();
    let resp = drain_resp(h.handle_blocking(req(
        "/?probe=sendfile-missing",
        "dispatcher/stream-worker.php",
    ))?);
    drop(h);
    r.shutdown();

    assert_eq!(resp.status(), 404);
    assert_eq!(resp.body_string(), "nope");
    Ok(())
}

/// A path outside the configured root is not sendable after symlink resolution.
#[test]
fn sendfile_outside_the_root_is_denied() -> anyhow::Result<()> {
    let _guard = php_lock();
    php_sys::set_sendfile_root(fixture("dispatcher"));
    let r = Rapira::start(Mode::Dispatcher(fixture("dispatcher/stream-worker.php")))?;
    let h = r.handle();
    let resp = drain_resp(h.handle_blocking(with_path_header(
        "/?probe=sendfile-escape",
        &fixture("shared/hello.php"),
    ))?);
    drop(h);
    r.shutdown();

    assert_eq!(resp.status(), 403);
    assert_eq!(resp.body_string(), "denied");
    Ok(())
}

// Checks writeTrailers in stream-worker.php as the third finalization operation.

/// Validated trailers use the `End` frame after streamed chunks.
#[test]
fn trailers_ride_the_end_frame() -> anyhow::Result<()> {
    let _guard = php_lock();
    let (r, h, rx) = stream_probe("/?probe=trailers")?;
    let resp = drain_resp(rx);
    drop(h);
    r.shutdown();

    assert_eq!(resp.status(), 200);
    assert_eq!(resp.body_string(), "chunk,");
    assert_eq!(
        resp.trailers,
        vec![("x-checksum".to_string(), b"abc123".to_vec())]
    );
    assert!(resp.ended && !resp.truncated);
    Ok(())
}

/// A response with only trailers uses Content-Length 0 and does not use empty chunked framing.
#[test]
fn trailers_only_response_keeps_length_framing() -> anyhow::Result<()> {
    let _guard = php_lock();
    let (r, h, rx) = stream_probe("/?probe=trailers-only")?;
    let resp = drain_resp(rx);
    drop(h);
    r.shutdown();

    assert_eq!(resp.status(), 200);
    assert_eq!(resp.content_length, Some(0));
    assert!(resp.body.is_empty());
    assert_eq!(
        resp.trailers,
        vec![("x-checksum".to_string(), b"empty".to_vec())]
    );
    Ok(())
}

/// Trailers before any head throw a catchable HeadNotWrittenError and the unit still serves.
#[test]
fn trailers_before_a_head_throw_head_not_written() -> anyhow::Result<()> {
    let _guard = php_lock();
    init_log_capture();
    captured().clear();
    let (r, h, rx) = stream_probe("/?probe=trailers-no-head")?;
    let resp = drain_resp(rx);

    assert_eq!(resp.status(), 200, "the handler recovers with a body");
    assert_eq!(resp.body_string(), "caught");
    let ctx = wait_app_record("trailers-no-head");
    assert!(ctx.contains("HeadNotWrittenError"), "wrong class in {ctx}");

    drop(h);
    r.shutdown();
    Ok(())
}

/// A field from a prohibited category raises `\ValueError` for all protocols. The handler continues after it catches the error.
#[test]
fn forbidden_trailer_field_is_rejected() -> anyhow::Result<()> {
    let _guard = php_lock();
    let (r, h, rx) = stream_probe("/?probe=trailers-forbidden")?;
    let resp = drain_resp(rx);
    drop(h);
    r.shutdown();

    assert_eq!(resp.status(), 200);
    assert_eq!(resp.body_string(), "rejected");
    Ok(())
}

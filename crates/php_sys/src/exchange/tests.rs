use super::headers::*;
use super::respond::*;
use super::sendfile::*;
use super::*;
use crate::types::{Context, Request};
use std::path::PathBuf;

fn base_req() -> Request {
    Request {
        method: String::new(),
        uri: "/".into(),
        target: None,
        authority: None,
        https: false,
        query: String::new(),
        protocol: String::new(),
        remote: Addr::Inet(([127, 0, 0, 1], 8080).into()),
        server: Addr::Inet(([127, 0, 0, 1], 8080).into()),
        server_name: String::new(),
        server_port: 8080,
        script_name: String::new(),
        document_root: String::new(),
        script_filename: PathBuf::new(),
        headers: Vec::new(),
        server_vars: Vec::new(),
        content_type: None,
        content_length: 0,
        body: Body::Raw(Box::new(std::io::empty())),
        received_at: None,
        tls: None,
    }
}

enum Sealed {
    Complete { status: u16, body: Vec<u8> },
    Truncated { status: Option<u16>, body: Vec<u8> },
    Nothing,
}

fn recv_sealed(rx: &mut tokio::sync::mpsc::Receiver<crate::types::Frame>) -> Sealed {
    let (mut status, mut body, mut saw_frames) = (None, Vec::new(), false);
    while let Ok(frame) = rx.try_recv() {
        saw_frames = true;
        match frame {
            crate::types::Frame::Interim(_) | crate::types::Frame::File { .. } => {}
            crate::types::Frame::Head { head, .. } => status = Some(head.status),
            crate::types::Frame::Chunk(b) => body.extend_from_slice(&b),
            crate::types::Frame::End { truncated, .. } => {
                return match (truncated, status) {
                    (true, status) => Sealed::Truncated { status, body },
                    (false, Some(status)) => Sealed::Complete { status, body },
                    (false, None) => Sealed::Nothing,
                };
            }
        }
    }
    if saw_frames {
        panic!("stream carried frames but no End");
    }
    Sealed::Nothing
}

/// The channel has capacity for a complete group of three events, so seal does not wait without a reader.
fn state_of(
    req: Request,
) -> (
    ExchangeState,
    tokio::sync::mpsc::Receiver<crate::types::Frame>,
) {
    let (tx, rx) = tokio::sync::mpsc::channel(64);
    let job = Box::new(Job {
        ctx: Context::new(req, tx, /*superglobals=*/ false),
    });
    let Ok(st) = ExchangeState::new(job) else {
        unreachable!("empty cursor body always reads")
    };
    (st, rx)
}

fn state() -> (
    ExchangeState,
    tokio::sync::mpsc::Receiver<crate::types::Frame>,
) {
    state_of(base_req())
}

/// A full response channel waits for its consumer. The wait does not require active PHP state.
#[test]
fn full_response_channel_waits_for_capacity() {
    use crate::types::Frame;

    let (mut st, old_rx) = state();
    drop(old_rx);
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    st.job.ctx.sender = Some(tx);
    assert!(send_frame(&mut st, Frame::Chunk(Bytes::from_static(b"first"))).is_ok());

    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let sender = std::thread::spawn(move || {
        started_tx.send(()).unwrap();
        let sent = send_frame(&mut st, Frame::Chunk(Bytes::from_static(b"second"))).is_ok();
        done_tx.send(sent).unwrap();
    });

    started_rx.recv().unwrap();
    assert!(
        done_rx.recv_timeout(Duration::from_millis(20)).is_err(),
        "the full channel must wait"
    );
    assert!(matches!(rx.blocking_recv(), Some(Frame::Chunk(bytes)) if bytes == b"first"[..]));
    assert!(done_rx.recv_timeout(Duration::from_secs(1)).unwrap());
    assert!(matches!(rx.blocking_recv(), Some(Frame::Chunk(bytes)) if bytes == b"second"[..]));
    sender.join().unwrap();
}

/// An overflow without sealing would leave the unit in `Handling`. The single-flight check would then reject all later receive() calls.
#[test]
fn overflow_seals_the_unit_truncated() {
    let (mut st, mut rx) = state();
    let v = unsafe { write_body_core(&mut st, c"x".as_ptr(), MAX_BUFFERED_BODY + 1, false) };
    assert_eq!(v, Verb::Overflow);

    let Sealed::Truncated { status, body } = recv_sealed(&mut rx) else {
        panic!("overflow must seal a truncated stream");
    };
    assert_eq!(status, Some(200));
    assert!(body.is_empty(), "the overflowing chunk is never sent");

    let v = unsafe { write_body_core(&mut st, c"y".as_ptr(), 1, true) };
    assert_eq!(v, Verb::Finalized);
    let job: *const c_void = (&raw const st).cast();
    assert!(unsafe { rapira_rs_exchange_is_finalized(job) });
}

/// A 304 head discards accepted body chunks during sealing. A 204 or `HEAD` response has the same behavior.
#[test]
fn seal_drops_the_body_for_304() {
    let (mut st, mut rx) = state();
    assert_eq!(write_head_core(&mut st, 304, Vec::new()), Verb::Ok);
    let v = unsafe { write_body_core(&mut st, c"gone".as_ptr(), 4, true) };
    assert_eq!(v, Verb::Ok);
    let Sealed::Complete { status, body } = recv_sealed(&mut rx) else {
        panic!("must seal cleanly");
    };
    assert_eq!(status, 304);
    assert!(body.is_empty(), "304 carries no body");
}

/// An empty chunk without `eos` does not commit a head.
#[test]
fn empty_non_eos_chunk_commits_nothing() {
    let (mut st, mut rx) = state();
    let v = unsafe { write_body_core(&mut st, c"".as_ptr(), 0, false) };
    assert_eq!(v, Verb::Ok);
    assert_eq!(
        write_head_core(&mut st, 404, Vec::new()),
        Verb::Ok,
        "the head slot must still be open"
    );
    let v = unsafe { write_body_core(&mut st, c"x".as_ptr(), 1, true) };
    assert_eq!(v, Verb::Ok);
    let Sealed::Complete { status, .. } = recv_sealed(&mut rx) else {
        panic!("must seal cleanly");
    };
    assert_eq!(status, 404);
}

/// The response validators use the same byte sets as the classic path.
#[test]
fn wire_validators_match_the_classic_byte_sets() {
    assert!(wire_token(b"x-trace"));
    assert!(!wire_token(b""));
    assert!(!wire_token(b"bad name"));
    assert!(!wire_token(b"x:y"));
    assert!(wire_value(b"a\tb \xff"));
    assert!(!wire_value(b"a\x01b"));
    assert!(!wire_value(b"a\x7fb"));
    assert!(!wire_value(b"split\r\nx: y"));
    assert!(!wire_value(b"nul\0"));
}

/// Construction normalizes the protocol spelling and maps an empty unix path to the unnamed endpoint.
#[test]
fn construction_normalizes_protocol_and_empty_unix_path() {
    let mut req = base_req();
    req.protocol = "HTTP/3.0".into();
    req.remote = Addr::Unix(Some(PathBuf::new()));
    let (st, _rx) = state_of(req);
    assert_eq!(st.protocol_php, "HTTP/3");
    assert!(matches!(st.remote, AddrOwned::Unix(None)));
}

/// A one-shot write includes its computed length in the `Head` frame. The HTTP server selects framing for a streamed write.
#[test]
fn head_frame_length_follows_the_write_shape() {
    use crate::types::Frame;
    let (mut st, mut rx) = state();
    let v = unsafe { write_body_core(&mut st, c"abc".as_ptr(), 3, true) };
    assert_eq!(v, Verb::Ok);
    let Ok(Frame::Head { content_length, .. }) = rx.try_recv() else {
        panic!("head first");
    };
    assert_eq!(content_length, Some(3));

    let (mut st, mut rx) = state();
    let v = unsafe { write_body_core(&mut st, c"abc".as_ptr(), 3, false) };
    assert_eq!(v, Verb::Ok);
    let Ok(Frame::Head { content_length, .. }) = rx.try_recv() else {
        panic!("head first");
    };
    assert_eq!(content_length, None, "streaming: the front frames");
}

/// If the declared length is too large, the function sends the available prefix and seals without truncation. Later writes receive `Finalized`.
#[test]
fn content_length_exceeded_sends_the_prefix_and_seals() {
    use crate::types::Frame;
    let (mut st, mut rx) = state();
    let v = write_head_core(&mut st, 200, vec![("content-length".into(), b"5".to_vec())]);
    assert_eq!(v, Verb::Ok);
    let v = unsafe { write_body_core(&mut st, c"0123456789".as_ptr(), 10, true) };
    assert_eq!(v, Verb::ContentLengthExceeded);

    let Ok(Frame::Head { content_length, .. }) = rx.try_recv() else {
        panic!("head first");
    };
    assert_eq!(content_length, Some(5), "the declared length is honoured");
    let Ok(Frame::Chunk(b)) = rx.try_recv() else {
        panic!("the fitting prefix must be sent");
    };
    assert_eq!(&b[..], b"01234");
    let Ok(Frame::End { truncated, .. }) = rx.try_recv() else {
        panic!("sealed");
    };
    assert!(!truncated, "complete per its declaration");

    let v = unsafe { write_body_core(&mut st, c"x".as_ptr(), 1, true) };
    assert_eq!(v, Verb::Finalized, "nothing written after it");
}

/// A repeated content-length in the head table is a `\ValueError`.
#[test]
fn repeated_content_length_is_a_bad_field() {
    let (mut st, _rx) = state();
    let v = write_head_core(
        &mut st,
        200,
        vec![
            ("content-length".into(), b"5".to_vec()),
            ("Content-Length".into(), b"7".to_vec()),
        ],
    );
    assert!(matches!(v, Verb::BadField(_)));
    assert_eq!(st.stage, Stage::Open, "a rejected head commits nothing");
}

/// An interim head is sent immediately without framing fields. A final head can still follow it.
#[test]
fn interim_head_emits_without_framing_fields() {
    use crate::types::Frame;
    let (mut st, mut rx) = state();
    let v = write_head_core(
        &mut st,
        103,
        vec![
            ("link".into(), b"</a.css>; rel=preload".to_vec()),
            ("content-length".into(), b"5".to_vec()),
            ("connection".into(), b"close".to_vec()),
        ],
    );
    assert_eq!(v, Verb::Interim);
    let Ok(Frame::Interim(head)) = rx.try_recv() else {
        panic!("interim head must be on the stream");
    };
    assert_eq!(head.status, 103);
    assert_eq!(
        head.headers.len(),
        1,
        "framing fields stripped: {:?}",
        head.headers
    );
    assert_eq!(head.headers[0].0, "link");
    let v = write_head_core(&mut st, 200, Vec::new());
    assert_eq!(v, Verb::Ok, "the final-head slot stays open");
}

/// A disconnected client discards the unit once. The unit remains finalized and cancelled.
#[test]
fn gone_client_discards_once_and_stays_discarded() {
    let (mut st, rx) = state();
    drop(rx);
    let v = unsafe { write_body_core(&mut st, c"x".as_ptr(), 1, false) };
    assert_eq!(v, Verb::Discarded);
    let v = unsafe { write_body_core(&mut st, c"y".as_ptr(), 1, true) };
    assert_eq!(v, Verb::Discarded, "sticky across repeat writes");

    let job: *const c_void = (&raw const st).cast();
    assert!(unsafe { rapira_rs_exchange_is_finalized(job) });
    assert!(unsafe { rapira_rs_exchange_is_cancelled(job) });
}

/// `flush()` sends the implicit 200 response once. A repeated call sends no additional data.
#[test]
fn flush_emits_the_implicit_head_once() {
    use crate::types::Frame;
    let (mut st, mut rx) = state();
    let job: *mut c_void = (&raw mut st).cast();
    assert!(unsafe { rapira_rs_exchange_flush(job) });
    let Ok(Frame::Head {
        head,
        content_length,
        ..
    }) = rx.try_recv()
    else {
        panic!("flush must emit the head");
    };
    assert_eq!(head.status, 200);
    assert!(head.headers.is_empty(), "implicit 200 has no fields");
    assert_eq!(content_length, None, "flush costs the computed length");
    assert!(unsafe { rapira_rs_exchange_flush(job) });
    assert!(rx.try_recv().is_err(), "a repeat flush is a no-op");
}

/// A committed 101 response has no body. The function accepts and discards chunks.
#[test]
fn a_101_head_drops_body_chunks() {
    use crate::types::Frame;
    let (mut st, mut rx) = state();
    assert_eq!(write_head_core(&mut st, 101, Vec::new()), Verb::Ok);
    let v = unsafe { write_body_core(&mut st, c"upgrade".as_ptr(), 7, true) };
    assert_eq!(v, Verb::Ok);
    let Ok(Frame::Head { bodiless, .. }) = rx.try_recv() else {
        panic!("head first");
    };
    assert!(bodiless);
    assert!(
        matches!(rx.try_recv(), Ok(Frame::End { .. })),
        "no chunk frames for a 1xx response"
    );
}

/// This test contains all sendFile validation because the root is process-global state.
#[test]
fn send_file_validation_table() {
    use crate::types::Frame;
    let test_dir = std::env::temp_dir().join(format!("rapira-sf-{}", std::process::id()));
    let dir = test_dir.join("root");
    let sibling = test_dir.join("root-other");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::create_dir_all(&sibling).unwrap();
    set_sendfile_root(dir.join("."));
    let path = dir.join("payload-\u{00e9}-\u{1f4c4}.txt");
    std::fs::write(&path, b"abcdefghijklmnopqrstuvwxyz").unwrap();
    let pb = path_bytes(&path);
    let outside = sibling.join("outside.txt");
    std::fs::write(&outside, b"outside").unwrap();
    std::fs::write(dir.join("\u{fffd}.txt"), b"replacement").unwrap();
    let mut invalid_utf8 = path_bytes(&dir);
    invalid_utf8.extend_from_slice(b"\\\xff.txt");

    let (mut st, mut rx) = state();
    let v = send_file_core(&mut st, &invalid_utf8, 0, None, true);
    assert_eq!(v, Verb::FileNotSendable(c"the path is not valid UTF-8"));
    for (name, path, offset, length) in [
        ("missing", path_bytes(&dir.join("missing")), 0, None),
        ("directory", path_bytes(&dir), 0, None),
        ("offset past end", pb.clone(), 27, None),
        ("slice past end", pb.clone(), 20, Some(10)),
        ("sibling prefix", path_bytes(&outside), 0, None),
        (
            "parent traversal",
            path_bytes(&dir.join("..\\root-other\\outside.txt")),
            0,
            None,
        ),
    ] {
        let v = send_file_core(&mut st, &path, offset, length, true);
        assert!(matches!(v, Verb::FileNotSendable(_)), "{name}");
    }
    assert_eq!(st.stage, Stage::Open);
    assert!(rx.try_recv().is_err(), "rejected paths send no frames");

    let (mut st, mut rx) = state();
    let v = send_file_core(&mut st, &pb, 2, Some(3), true);
    assert_eq!(v, Verb::Ok);
    let Ok(Frame::Head { content_length, .. }) = rx.try_recv() else {
        panic!("head first");
    };
    assert_eq!(
        content_length,
        Some(3),
        "the slice length is known up front"
    );
    let Ok(Frame::File { offset, len, .. }) = rx.try_recv() else {
        panic!("the file rides its own frame");
    };
    assert_eq!((offset, len), (2, 3));
    assert!(matches!(
        rx.try_recv(),
        Ok(Frame::End {
            truncated: false,
            ..
        })
    ));
    assert_eq!(st.stage, Stage::Finalized);

    let verbatim = std::fs::canonicalize(&path).unwrap();
    let (mut st, rx) = state();
    let v = send_file_core(&mut st, &path_bytes(&verbatim), 0, None, true);
    assert_eq!(v, Verb::Ok, "verbatim paths use the canonical root");
    drop(rx);

    let link_out = dir.join("link-out.txt");
    let link_in = dir.join("link-in.txt");
    for (target, link) in [(&outside, &link_out), (&path, &link_in)] {
        if let Err(error) = std::os::windows::fs::symlink_file(target, link) {
            const ERROR_PRIVILEGE_NOT_HELD: i32 = 1314;
            if error.raw_os_error() == Some(ERROR_PRIVILEGE_NOT_HELD) {
                std::fs::remove_dir_all(&test_dir).unwrap();
                return;
            }
            panic!("cannot create the test symlink: {error}");
        }
    }
    let (mut st, _rx) = state();
    let v = send_file_core(&mut st, &path_bytes(&link_out), 0, None, true);
    assert!(matches!(v, Verb::FileNotSendable(_)), "escaping symlink");

    let (mut st, mut rx) = state();
    let v = send_file_core(&mut st, &path_bytes(&link_in), 0, None, true);
    assert_eq!(v, Verb::Ok, "intra-root symlinks stay sendable");
    assert!(matches!(rx.try_recv(), Ok(Frame::Head { .. })));
    drop(rx);

    std::fs::remove_dir_all(&test_dir).unwrap();
}

/// Trailers end the response in the `End` frame. A call without a head returns `HeadNotWritten`, and a repeated call returns `Finalized`.
#[test]
fn trailers_finalize_with_a_committed_head() {
    use crate::types::Frame;
    let (mut st, mut rx) = state();
    let v = write_trailers_core(&mut st, vec![("x".into(), b"y".to_vec())]);
    assert_eq!(v, Verb::HeadNotWritten, "nothing here commits a head");

    assert_eq!(write_head_core(&mut st, 200, Vec::new()), Verb::Ok);
    let v = write_trailers_core(&mut st, vec![("x".into(), b"y".to_vec())]);
    assert_eq!(v, Verb::Ok);
    let Ok(Frame::Head { content_length, .. }) = rx.try_recv() else {
        panic!("head first");
    };
    assert_eq!(
        content_length,
        Some(0),
        "trailers-only keeps length framing"
    );
    let Ok(Frame::End {
        trailers,
        truncated,
    }) = rx.try_recv()
    else {
        panic!("the trailers ride the End frame");
    };
    assert!(!truncated);
    assert_eq!(trailers, vec![("x".to_string(), b"y".to_vec())]);

    let v = write_trailers_core(&mut st, Vec::new());
    assert_eq!(v, Verb::Finalized);
}

/// The prohibited set includes every category from RFC 9110 section 6.5.1. The function permits unknown extension fields. https://www.rfc-editor.org/rfc/rfc9110#section-6.5.1
#[test]
fn trailer_denylist_matches_the_categories() {
    for name in [
        "Content-Length",
        "connection",
        "host",
        "authorization",
        "cache-control",
        "date",
        "content-type",
    ] {
        assert!(forbidden_trailer(name), "{name}");
    }
    assert!(!forbidden_trailer("x-checksum"));
    assert!(!forbidden_trailer("server-timing"));
}

/// Sealing unlinks the spool files, so uploads do not exist after the exchange is finalized.
#[test]
fn seal_unlinks_the_spool_files() {
    let (mut st, mut _rx) = state();
    let dir = std::env::temp_dir();
    let path = dir.join(format!("rapira-test-spool-{}", std::process::id()));
    std::fs::write(&path, b"payload").unwrap();
    st.body = BodyState::Multipart {
        fields: Vec::new(),
        files: vec![FilePart {
            upload: crate::types::UploadedFile {
                name: b"f".to_vec(),
                client_filename: b"a.bin".to_vec(),
                client_media_type: None,
                headers: Vec::new(),
                file: crate::types::SpooledFile { path: path.clone() },
                size: 7,
            },
            path: path_bytes(&path),
            headers: Grouped::new(&[]),
        }],
    };
    assert!(path.exists());
    seal(&mut st, false, Vec::new());
    assert!(!path.exists(), "seal must unlink the spooled file");
}

//! The rapira_grpc server over the wire, with `grpc/wire-worker.php` in the role of the application: the three protocols, statuses and metadata, what PHP sees, deadlines, the plugin routes, and the drain.

use std::convert::Infallible;
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use http::Method;
use http_body_util::BodyExt;
use hyper::body::{Body, Bytes, Frame};
use hyper_util::rt::{TokioExecutor, TokioIo};
use serde_json::{Value, json};
use tests::grpc::{
    Conn, ECHO_PATH, ERROR_INFO, Fields, HI, HI_FRAME, Response, Wire, envelope, fields,
    status_bytes, status_details, web_trailers,
};
use tests::{fixture, server_log};

use crate::harness::{
    Server, Spawn, calls, exited, http_raw, listen, logged_remote, send_ctrl_break, stop,
};

/// Logs a `call` record for each call and answers as the request metadata asks.
fn wire_worker() -> PathBuf {
    fixture("grpc/wire-worker.php")
}

/// The error that the host logs when the gRPC plugin stops.
fn plugin_error(srv: &Server) -> Option<String> {
    server_log::records(&srv.log_file())
        .into_iter()
        .find(|c| c.target == "rapira" && c.message.starts_with("grpc drain timed out"))
        .map(|c| c.message)
}

#[derive(Clone, Copy)]
enum On {
    Tcp,
}

/// Received bodies share the 64 MiB pool budget. A temporary refusal is UNAVAILABLE: https://connectrpc.com/docs/protocol/#error-codes.
#[tokio::test]
async fn incomplete_rpc_bodies_share_the_pool_budget() {
    struct Incomplete(Option<Bytes>);
    impl Body for Incomplete {
        type Data = Bytes;
        type Error = Infallible;

        fn poll_frame(
            mut self: Pin<&mut Self>,
            _: &mut Context<'_>,
        ) -> Poll<Option<Result<Frame<Bytes>, Infallible>>> {
            match self.0.take() {
                Some(bytes) => Poll::Ready(Some(Ok(Frame::data(bytes)))),
                None => Poll::Pending,
            }
        }
    }
    struct Case {
        name: &'static str,
        content_type: &'static str,
    }
    let cases = [
        Case {
            name: "supported content type",
            content_type: "application/proto",
        },
        Case {
            name: "unsupported content type",
            content_type: "application/unknown",
        },
    ];
    let srv = Spawn::grpc(wire_worker())
        .grpc_extra("default_timeout_secs = 10\nmax_timeout_secs = 10")
        .json_log()
        .spawn();
    let addr = listen(&srv);
    let data = Bytes::from(vec![b'x'; 4 * 1024 * 1024]);
    for case in cases {
        let before = calls(&srv).len();
        let mut requests = tokio::task::JoinSet::new();
        let mut connections = Vec::new();
        for _ in 0..17 {
            let tcp = tokio::net::TcpStream::connect(addr).await.expect("connect");
            let (mut send, connection) =
                hyper::client::conn::http2::handshake(TokioExecutor::new(), TokioIo::new(tcp))
                    .await
                    .expect("h2 handshake");
            connections.push(tokio::spawn(connection));
            let request = http::Request::post(format!("http://localhost{ECHO_PATH}"))
                .header("content-type", case.content_type)
                .body(Incomplete(Some(data.clone())))
                .expect("request");
            requests.spawn(async move {
                let (parts, body) = send.send_request(request).await?.into_parts();
                anyhow::Ok((parts.status, body.collect().await?.to_bytes()))
            });
        }
        let (status, body) = tokio::time::timeout(Duration::from_secs(5), requests.join_next())
            .await
            .unwrap_or_else(|_| panic!("{}: the body budget did not refuse a request", case.name))
            .expect("a pending request")
            .expect("request task")
            .expect("response");
        assert_eq!(
            status,
            http::StatusCode::SERVICE_UNAVAILABLE,
            "{}",
            case.name
        );
        let error: Value = serde_json::from_slice(&body).expect("Connect error");
        assert_eq!(error["code"], "unavailable", "{}", case.name);
        assert_eq!(
            calls(&srv).len(),
            before,
            "{}: no incomplete call reaches PHP",
            case.name
        );

        requests.shutdown().await;
        for connection in connections {
            connection.abort();
            let _ = connection.await;
        }
        let mut conn = Conn::open(&addr, Wire::H2).await.expect("connect");
        let got = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let got = conn
                    .send(
                        Method::POST,
                        ECHO_PATH,
                        &[("content-type", "application/proto")],
                        HI,
                    )
                    .await
                    .expect("response");
                if got.status != 503 {
                    break got;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("{}: cancelled requests retained the budget", case.name));
        assert_eq!(got.status, 200, "{}", case.name);
        assert_eq!(got.body.as_ref(), HI, "{}", case.name);
    }
    stop(srv).await;
}

#[tokio::test]
async fn declared_rpc_body_lengths_do_not_consume_the_pool_budget() {
    let srv = Spawn::grpc(wire_worker())
        .grpc_extra("default_timeout_secs = 10\nmax_timeout_secs = 10")
        .json_log()
        .spawn();
    let addr = listen(&srv);
    let mut uploads = Vec::new();
    for _ in 0..17 {
        let mut stream = TcpStream::connect(addr).expect("connect");
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        write!(stream, "POST {ECHO_PATH} HTTP/1.1\r\nHost: e2e\r\nContent-Type: application/proto\r\nContent-Length: 4194304\r\nExpect: 100-continue\r\n\r\n").expect("request head");
        let mut head = Vec::new();
        while !head.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            stream.read_exact(&mut byte).expect("interim response");
            head.push(byte[0]);
        }
        assert!(
            head.starts_with(b"HTTP/1.1 100 "),
            "{}",
            String::from_utf8_lossy(&head)
        );
        uploads.push(stream);
    }
    let mut conn = Conn::open(&addr, Wire::Http1).await.expect("connect");
    let got = conn
        .send(
            Method::POST,
            ECHO_PATH,
            &[("content-type", "application/proto")],
            HI,
        )
        .await
        .expect("response");
    assert_eq!(got.status, 200);
    assert_eq!(got.body.as_ref(), HI);
    assert_eq!(
        calls(&srv).len(),
        1,
        "only the complete request reaches PHP"
    );
    drop(uploads);
    stop(srv).await;
}

/// JSON elements use the existing 32 MiB decode budget. At 64 bytes per dynamic element, 600,000 int32 values exceed it: https://github.com/anthropics/buffa/blob/104716922c66e21cf26895a2a5a2a42e6292f9d4/buffa-descriptor/src/reflect/json.rs.
#[tokio::test]
async fn json_request_elements_have_a_memory_limit() {
    struct Case {
        name: &'static str,
        body: String,
        status: u16,
        expected: Value,
    }
    let cases = [
        Case {
            name: "small repeated field with an unknown field",
            body: r#"{"ids":[0,1],"unknown":[2,3]}"#.to_owned(),
            status: 200,
            expected: json!({"ids": [0, 1]}),
        },
        Case {
            name: "repeated field above the element budget",
            body: format!(r#"{{"ids":[{}0]}}"#, "0,".repeat(599_999)),
            status: 429,
            expected: json!("resource_exhausted"),
        },
        Case {
            name: "invalid repeated field value",
            body: r#"{"ids":["invalid"]}"#.to_owned(),
            status: 400,
            expected: json!("invalid_argument"),
        },
    ];
    let srv = Spawn::grpc(wire_worker()).json_log().spawn();
    for case in cases {
        let before = calls(&srv).len();
        let mut conn = Conn::open(&listen(&srv), Wire::Http1)
            .await
            .expect("connect");
        let got = conn
            .send(
                Method::POST,
                ECHO_PATH,
                &[("content-type", "application/json")],
                case.body.as_bytes(),
            )
            .await
            .unwrap_or_else(|e| panic!("{}: {e:#}", case.name));
        assert_eq!(got.status, case.status, "{}", case.name);
        let reply: Value = serde_json::from_slice(&got.body).expect("JSON response");
        let actual = if case.status == 200 {
            &reply
        } else {
            &reply["code"]
        };
        assert_eq!(actual, &case.expected, "{}", case.name);
        assert_eq!(
            calls(&srv).len() - before,
            usize::from(case.status == 200),
            "{}",
            case.name
        );
    }
    stop(srv).await;
}

/// The message limit is 4 MiB. The gRPC envelope adds five bytes: https://github.com/grpc/grpc/blob/master/doc/PROTOCOL-HTTP2.md#requests. RESOURCE_EXHAUSTED is HTTP 429 for Connect: https://connectrpc.com/docs/protocol/#error-codes.
#[tokio::test]
async fn request_message_limit_excludes_the_envelope() {
    struct Case {
        name: &'static str,
        wire: Wire,
        content_type: &'static str,
        bytes: usize,
        status: u16,
        grpc_status: Option<&'static str>,
    }
    const LIMIT: usize = 4 * 1024 * 1024;
    let cases = [
        Case {
            name: "connect at the message limit",
            wire: Wire::Http1,
            content_type: "application/proto",
            bytes: LIMIT,
            status: 200,
            grpc_status: None,
        },
        Case {
            name: "connect above the message limit",
            wire: Wire::Http1,
            content_type: "application/proto",
            bytes: LIMIT + 1,
            status: 429,
            grpc_status: None,
        },
        Case {
            name: "grpc at the message limit",
            wire: Wire::H2,
            content_type: "application/grpc",
            bytes: LIMIT,
            status: 200,
            grpc_status: Some("0"),
        },
        Case {
            name: "grpc above the message limit",
            wire: Wire::H2,
            content_type: "application/grpc",
            bytes: LIMIT + 1,
            status: 200,
            grpc_status: Some("8"),
        },
        Case {
            name: "grpc-web at the message limit",
            wire: Wire::Http1,
            content_type: "application/grpc-web+proto",
            bytes: LIMIT,
            status: 200,
            grpc_status: Some("0"),
        },
        Case {
            name: "grpc-web above the message limit",
            wire: Wire::Http1,
            content_type: "application/grpc-web+proto",
            bytes: LIMIT + 1,
            status: 200,
            grpc_status: Some("8"),
        },
    ];
    let srv = Spawn::grpc(wire_worker()).json_log().spawn();
    for case in cases {
        let mut message = vec![0x0a];
        let mut length = case.bytes - 5;
        while length >= 128 {
            message.push((length as u8 & 0x7f) | 0x80);
            length >>= 7;
        }
        message.push(length as u8);
        message.resize(case.bytes, b'x');
        let body = if case.grpc_status.is_some() {
            envelope(&message)
        } else {
            message
        };
        let mut conn = Conn::open(&listen(&srv), case.wire).await.expect("connect");
        let got = conn
            .send(
                Method::POST,
                ECHO_PATH,
                &[
                    ("content-type", case.content_type),
                    ("te", "trailers"),
                    ("x-reply", "ff"),
                ],
                &body,
            )
            .await
            .unwrap_or_else(|e| panic!("{}: {e:#}", case.name));
        assert_eq!(got.status, case.status, "{}", case.name);
        if let Some(status) = case.grpc_status {
            let trailers = web_trailers(&got.body);
            let actual = if case.content_type == "application/grpc-web+proto" {
                trailers.get("grpc-status").and_then(|v| v.to_str().ok())
            } else {
                got.grpc_status()
            };
            assert_eq!(actual, Some(status), "{}", case.name);
        } else if case.bytes > LIMIT {
            let error: Value = serde_json::from_slice(&got.body).expect("Connect error");
            assert_eq!(error["code"], "resource_exhausted", "{}", case.name);
        } else {
            assert_eq!(got.body.as_ref(), b"\xff", "{}", case.name);
        }
    }
    assert_eq!(
        calls(&srv).len(),
        3,
        "only messages within the limit reach PHP"
    );
    stop(srv).await;
}

/// Sources: PROTOCOL-HTTP2 (envelope, `grpc-status`, 12 for an unknown method or encoding), PROTOCOL-WEB (the 0x80 trailer frame), the Connect protocol (unary request, GET for no-side-effects methods, 404 for an unknown procedure), and proto3 JSON.
#[tokio::test]
async fn each_protocol_reaches_php_over_the_wire() {
    struct Case {
        name: &'static str,
        on: On,
        wire: Wire,
        method: Method,
        path: &'static str,
        headers: Fields,
        body: &'static [u8],
        status: u16,
        grpc_status: Option<&'static str>,
        content_type: Option<&'static str>,
        /// None: not checked.
        reply: Option<&'static [u8]>,
        /// The call reaches PHP.
        php: bool,
    }
    let grpc: Fields = &[("content-type", "application/grpc"), ("te", "trailers")];
    let cases = [
        Case {
            name: "grpc over h2c tcp",
            on: On::Tcp,
            wire: Wire::H2,
            method: Method::POST,
            path: ECHO_PATH,
            headers: grpc,
            body: HI_FRAME,
            status: 200,
            grpc_status: Some("0"),
            content_type: None,
            reply: Some(HI_FRAME),
            php: true,
        },
        Case {
            name: "grpc-web binary over http/1.1",
            on: On::Tcp,
            wire: Wire::Http1,
            method: Method::POST,
            path: ECHO_PATH,
            headers: &[("content-type", "application/grpc-web+proto")],
            body: HI_FRAME,
            status: 200,
            grpc_status: None,
            content_type: None,
            reply: Some(b"\x00\x00\x00\x00\x04\x0a\x02hi\x80\x00\x00\x00\x10grpc-status: 0\r\n"),
            php: true,
        },
        Case {
            name: "connect proto",
            on: On::Tcp,
            wire: Wire::Http1,
            method: Method::POST,
            path: ECHO_PATH,
            headers: &[("content-type", "application/proto")],
            body: HI,
            status: 200,
            grpc_status: None,
            content_type: Some("application/proto"),
            reply: Some(HI),
            php: true,
        },
        Case {
            name: "connect json",
            on: On::Tcp,
            wire: Wire::Http1,
            method: Method::POST,
            path: ECHO_PATH,
            headers: &[("content-type", "application/json")],
            body: br#"{"text":"hi"}"#,
            status: 200,
            grpc_status: None,
            content_type: Some("application/json"),
            reply: Some(br#"{"text":"hi"}"#),
            php: true,
        },
        Case {
            name: "connect get, idempotent",
            on: On::Tcp,
            wire: Wire::Http1,
            method: Method::GET,
            path: "/rapira.test.v1.EchoService/Get?encoding=json&message=%7B%22text%22%3A%22hi%22%7D",
            headers: &[],
            body: b"",
            status: 200,
            grpc_status: None,
            content_type: Some("application/json"),
            reply: Some(br#"{"text":"hi"}"#),
            php: true,
        },
        Case {
            name: "connect get, side effects",
            on: On::Tcp,
            wire: Wire::Http1,
            method: Method::GET,
            path: "/rapira.test.v1.EchoService/Echo?encoding=json&message=%7B%22text%22%3A%22hi%22%7D",
            headers: &[],
            body: b"",
            status: 405,
            grpc_status: None,
            content_type: None,
            reply: None,
            php: false,
        },
        Case {
            name: "streaming method",
            on: On::Tcp,
            wire: Wire::H2,
            method: Method::POST,
            path: "/rapira.test.v1.EchoService/Watch",
            headers: grpc,
            body: HI_FRAME,
            status: 200,
            grpc_status: Some("12"),
            content_type: None,
            reply: Some(b""),
            php: false,
        },
        Case {
            name: "unlisted service",
            on: On::Tcp,
            wire: Wire::H2,
            method: Method::POST,
            path: "/rapira.test.v1.OtherService/Ping",
            headers: grpc,
            body: HI_FRAME,
            status: 200,
            grpc_status: Some("12"),
            content_type: None,
            reply: Some(b""),
            php: false,
        },
        Case {
            name: "unknown method over grpc",
            on: On::Tcp,
            wire: Wire::H2,
            method: Method::POST,
            path: "/x.Y/Z",
            headers: grpc,
            body: HI_FRAME,
            status: 200,
            grpc_status: Some("12"),
            content_type: None,
            reply: Some(b""),
            php: false,
        },
        Case {
            name: "unknown method over connect",
            on: On::Tcp,
            wire: Wire::Http1,
            method: Method::POST,
            path: "/x.Y/Z",
            headers: &[("content-type", "application/proto")],
            body: HI,
            status: 404,
            grpc_status: None,
            content_type: None,
            reply: None,
            php: false,
        },
        Case {
            name: "zstd request",
            on: On::Tcp,
            wire: Wire::H2,
            method: Method::POST,
            path: ECHO_PATH,
            headers: &[
                ("content-type", "application/grpc"),
                ("te", "trailers"),
                ("grpc-encoding", "zstd"),
            ],
            body: b"\x01\x00\x00\x00\x04\x28\xb5\x2f\xfd",
            status: 200,
            grpc_status: Some("12"),
            content_type: None,
            reply: Some(b""),
            php: false,
        },
    ];

    let on_tcp = Spawn::grpc(wire_worker()).json_log().spawn();
    let tcp = listen(&on_tcp);

    for case in cases {
        let (listen, srv) = match case.on {
            On::Tcp => (&tcp, &on_tcp),
        };
        let before = calls(srv).len();
        let mut conn = Conn::open(listen, case.wire).await.expect("connect");
        let got = conn
            .send(case.method, case.path, case.headers, case.body)
            .await
            .unwrap_or_else(|e| panic!("{}: {e:#}", case.name));

        assert_eq!(got.status, case.status, "{}: {got:?}", case.name);
        assert_eq!(
            got.grpc_status(),
            case.grpc_status,
            "{}: {got:?}",
            case.name
        );
        if let Some(content_type) = case.content_type {
            assert_eq!(
                got.headers.get("content-type").map(|v| v.as_bytes()),
                Some(content_type.as_bytes()),
                "{}",
                case.name
            );
        }
        if let Some(reply) = case.reply {
            assert_eq!(&got.body[..], reply, "{}", case.name);
        }
        let remotes: Vec<Value> = calls(srv)[before..]
            .iter()
            .map(|call| call["remote"].clone())
            .collect();
        let want: Vec<Value> = if case.php {
            vec![json!(logged_remote(&conn.peer))]
        } else {
            Vec::new()
        };
        assert_eq!(remotes, want, "{}: the calls that reached PHP", case.name);
    }

    stop(on_tcp).await;
}

#[tokio::test]
async fn reply_compression_follows_the_client() {
    struct Case<'a> {
        name: &'static str,
        wire: Wire,
        method: Method,
        path: &'a str,
        headers: Fields,
        body: &'a [u8],
        grpc_status: Option<&'static str>,
        compressed: bool,
        vary: Option<&'static str>,
    }
    let large = [&[0x0a, 0x80, 0x10][..], &[b'a'; 2048]].concat();
    let large_frame = envelope(&large);
    // The same protobuf message in a gzip envelope from the core regression test.
    let large_gzip: &[u8] = b"\x01\x00\x00\x00\x25\x1f\x8b\x08\x00\x00\x00\x00\x00\x02\xff\xe3\x6a\x10\x48\x1c\x05\xa3\x60\x14\x8c\x82\x51\x30\x0a\x46\xc1\x88\x03\x00\x93\x5d\xf0\xa7\x03\x08\x00\x00";
    let large_json = format!(r#"{{"text":"{}"}}"#, "a".repeat(2048));
    let get = format!(
        "/rapira.test.v1.EchoService/Get?encoding=json&message=%7B%22text%22%3A%22{}%22%7D",
        "a".repeat(2048)
    );
    let cases = [
        Case {
            name: "grpc accepts gzip with a plain request",
            wire: Wire::H2,
            method: Method::POST,
            path: ECHO_PATH,
            headers: &[
                ("content-type", "application/grpc"),
                ("te", "trailers"),
                ("grpc-accept-encoding", "gzip"),
            ],
            body: &large_frame,
            grpc_status: Some("0"),
            compressed: false,
            vary: None,
        },
        Case {
            name: "grpc gzip request",
            wire: Wire::H2,
            method: Method::POST,
            path: ECHO_PATH,
            headers: &[
                ("content-type", "application/grpc"),
                ("te", "trailers"),
                ("grpc-encoding", "gzip"),
                ("grpc-accept-encoding", "gzip"),
            ],
            body: large_gzip,
            grpc_status: Some("0"),
            compressed: true,
            vary: None,
        },
        Case {
            name: "grpc-web identity request",
            wire: Wire::Http1,
            method: Method::POST,
            path: ECHO_PATH,
            headers: &[
                ("content-type", "application/grpc-web+proto"),
                ("grpc-encoding", "identity"),
                ("grpc-accept-encoding", "gzip"),
            ],
            body: &large_frame,
            grpc_status: Some("0"),
            compressed: false,
            vary: None,
        },
        Case {
            name: "grpc-web gzip request",
            wire: Wire::Http1,
            method: Method::POST,
            path: ECHO_PATH,
            headers: &[
                ("content-type", "application/grpc-web+proto"),
                ("grpc-encoding", "gzip"),
                ("grpc-accept-encoding", "gzip"),
            ],
            body: large_gzip,
            grpc_status: Some("0"),
            compressed: true,
            vary: None,
        },
        Case {
            name: "Connect proto accepts gzip",
            wire: Wire::Http1,
            method: Method::POST,
            path: ECHO_PATH,
            headers: &[
                ("content-type", "application/proto"),
                ("accept-encoding", "gzip"),
            ],
            body: &large,
            grpc_status: None,
            compressed: true,
            vary: Some("accept-encoding"),
        },
        Case {
            name: "Connect JSON without gzip",
            wire: Wire::Http1,
            method: Method::POST,
            path: ECHO_PATH,
            headers: &[("content-type", "application/json")],
            body: large_json.as_bytes(),
            grpc_status: None,
            compressed: false,
            vary: Some("accept-encoding"),
        },
        Case {
            name: "Connect GET accepts gzip",
            wire: Wire::Http1,
            method: Method::GET,
            path: &get,
            headers: &[("accept-encoding", "gzip")],
            body: b"",
            grpc_status: None,
            compressed: true,
            vary: Some("accept-encoding"),
        },
        Case {
            name: "Connect reply below compression threshold",
            wire: Wire::Http1,
            method: Method::POST,
            path: ECHO_PATH,
            headers: &[
                ("content-type", "application/proto"),
                ("accept-encoding", "gzip"),
            ],
            body: HI,
            grpc_status: None,
            compressed: false,
            vary: Some("accept-encoding"),
        },
    ];
    let srv = Spawn::grpc(wire_worker()).spawn();
    for case in cases {
        let mut conn = Conn::open(&srv.addr, case.wire).await.expect(case.name);
        let got = conn
            .send(case.method, case.path, case.headers, case.body)
            .await
            .expect(case.name);
        assert_eq!(got.status, 200, "{}", case.name);
        let content_type = case
            .headers
            .iter()
            .find(|(name, _)| *name == "content-type")
            .map_or("", |(_, value)| *value);
        let web = (content_type == "application/grpc-web+proto").then(|| web_trailers(&got.body));
        let status = match &web {
            Some(web) => web.get("grpc-status").and_then(|v| v.to_str().ok()),
            None => got.grpc_status(),
        };
        assert_eq!(status, case.grpc_status, "{}", case.name);
        let compressed = if content_type.starts_with("application/grpc") {
            got.body.first() == Some(&1)
        } else {
            got.headers
                .get("content-encoding")
                .is_some_and(|v| v == "gzip")
        };
        assert_eq!(compressed, case.compressed, "{}", case.name);
        assert_eq!(
            got.headers.get("vary").map(|v| v.as_bytes()),
            case.vary.map(str::as_bytes),
            "{}",
            case.name
        );
    }
    stop(srv).await;
}

/// Sources: PROTOCOL-HTTP2 and PROTOCOL-WEB (the status fields, `grpc-status-details-bin`, Response-Headers without a status field), the Connect protocol (error codes, unpadded base64 detail values, `trailer-` fields of a unary response). connectrpc adds `type.googleapis.com/` to a bare name, so gRPC keeps a type URL whole and Connect sends the bare name.
#[tokio::test]
async fn statuses_and_metadata_encode_per_protocol() {
    struct Case {
        name: &'static str,
        /// PHP fails the call with NOT_FOUND and a detail of this type URL. None: PHP echoes the message. Both carry the halves.
        fails_with: Option<&'static str>,
        wire: Wire,
        content_type: &'static str,
        body: &'static [u8],
        status: u16,
        headers: Fields,
        /// The status fields: HTTP trailers for gRPC, the 0x80 frame for gRPC-Web.
        trailers: Fields,
        /// The type URL of the detail that `grpc-status-details-bin` carries. None: no such field.
        details: Option<&'static str>,
        /// None: not checked.
        reply: Option<&'static [u8]>,
    }
    const ACME: &str = "example.com/acme.Detail";
    let failed: Fields = &[
        ("grpc-status", "5"),
        ("grpc-message", "no invoice"),
        ("x-t", "w"),
        ("x-b-bin", "AQI"),
    ];
    let succeeded: Fields = &[("grpc-status", "0"), ("x-t", "w"), ("x-b-bin", "AQI")];
    let connect_halves: Fields = &[
        ("x-h", "v"),
        ("trailer-x-t", "w"),
        ("trailer-x-b-bin", "AQI"),
    ];
    let cases = [
        Case {
            name: "grpc error",
            fails_with: Some(ERROR_INFO),
            wire: Wire::H2,
            content_type: "application/grpc",
            body: HI_FRAME,
            status: 200,
            headers: &[("x-h", "v")],
            trailers: failed,
            details: Some(ERROR_INFO),
            reply: Some(b""),
        },
        Case {
            name: "grpc-web error",
            fails_with: Some(ERROR_INFO),
            wire: Wire::Http1,
            content_type: "application/grpc-web+proto",
            body: HI_FRAME,
            status: 200,
            headers: &[("x-h", "v")],
            trailers: failed,
            details: Some(ERROR_INFO),
            reply: None,
        },
        Case {
            name: "connect error",
            fails_with: Some(ERROR_INFO),
            wire: Wire::Http1,
            content_type: "application/proto",
            body: HI,
            status: 404,
            headers: connect_halves,
            trailers: &[],
            details: None,
            reply: Some(
                br#"{"code":"not_found","message":"no invoice","details":[{"type":"google.rpc.ErrorInfo","value":"CgF4"}]}"#,
            ),
        },
        Case {
            name: "grpc keeps a custom type url whole",
            fails_with: Some(ACME),
            wire: Wire::H2,
            content_type: "application/grpc",
            body: HI_FRAME,
            status: 200,
            headers: &[("x-h", "v")],
            trailers: failed,
            details: Some(ACME),
            reply: Some(b""),
        },
        Case {
            name: "connect sends the bare type name",
            fails_with: Some(ACME),
            wire: Wire::Http1,
            content_type: "application/proto",
            body: HI,
            status: 404,
            headers: connect_halves,
            trailers: &[],
            details: None,
            reply: Some(
                br#"{"code":"not_found","message":"no invoice","details":[{"type":"acme.Detail","value":"CgF4"}]}"#,
            ),
        },
        Case {
            name: "grpc success",
            fails_with: None,
            wire: Wire::H2,
            content_type: "application/grpc",
            body: HI_FRAME,
            status: 200,
            headers: &[("x-h", "v")],
            trailers: succeeded,
            details: None,
            reply: Some(HI_FRAME),
        },
        Case {
            name: "grpc-web success",
            fails_with: None,
            wire: Wire::Http1,
            content_type: "application/grpc-web+proto",
            body: HI_FRAME,
            status: 200,
            headers: &[("x-h", "v")],
            trailers: succeeded,
            details: None,
            reply: None,
        },
        Case {
            name: "connect success",
            fails_with: None,
            wire: Wire::Http1,
            content_type: "application/proto",
            body: HI,
            status: 200,
            headers: connect_halves,
            trailers: &[],
            details: None,
            reply: Some(HI),
        },
    ];

    let srv = Spawn::grpc(wire_worker()).spawn();
    for case in cases {
        let mut conn = Conn::open(&listen(&srv), case.wire).await.expect("connect");
        let mut headers = vec![
            ("content-type", case.content_type),
            ("te", "trailers"),
            ("x-halves", "1"),
        ];
        headers.extend(case.fails_with.map(|url| ("x-fail", url)));
        let got = conn
            .send(Method::POST, ECHO_PATH, &headers, case.body)
            .await
            .unwrap_or_else(|e| panic!("{}: {e:#}", case.name));

        assert_eq!(got.status, case.status, "{}: {got:?}", case.name);
        let trailers = if case.content_type.starts_with("application/grpc-web") {
            web_trailers(&got.body)
        } else {
            got.trailers.clone()
        };
        for (name, want) in fields(case.headers).iter() {
            assert_eq!(
                got.headers.get(name),
                Some(want),
                "{}: {name}: {got:?}",
                case.name
            );
        }
        for name in ["grpc-status", "grpc-message"] {
            assert!(
                !got.headers.contains_key(name),
                "{}: {name} in the head: {got:?}",
                case.name
            );
        }
        for (name, want) in fields(case.trailers).iter() {
            assert_eq!(
                trailers.get(name),
                Some(want),
                "{}: {name}: {trailers:?}",
                case.name
            );
        }
        assert_eq!(
            status_details(&trailers),
            case.details.map(status_bytes),
            "{}",
            case.name
        );
        if let Some(reply) = case.reply {
            assert_eq!(
                &got.body[..],
                reply,
                "{}: {}",
                case.name,
                String::from_utf8_lossy(&got.body)
            );
        }
    }
    stop(srv).await;
}

/// Sources: the contract gRPC README (a lost call is a sanitized INTERNAL), PROTOCOL-HTTP2 (3 is INVALID_ARGUMENT, 13 INTERNAL) and the Connect protocol (the HTTP status of each code).
#[tokio::test]
async fn php_failures_map_to_statuses() {
    /// What the PHP side does with the call.
    enum PhpDoes {
        /// Drops the call without an outcome.
        Lose,
        /// Replies with the byte ff.
        ReplyFf,
        /// The call never reaches PHP.
        Unreached,
    }
    struct Case {
        name: &'static str,
        php: PhpDoes,
        wire: Wire,
        content_type: &'static str,
        body: &'static [u8],
        status: u16,
        grpc_status: Option<&'static str>,
        /// The `code` of a Connect error body.
        connect_code: Option<&'static str>,
        /// The `grpc-message` field or the Connect `message`. None: not checked.
        message: Option<&'static str>,
        /// None: not checked.
        reply: Option<&'static [u8]>,
    }
    let cases = [
        Case {
            name: "a lost call is internal",
            php: PhpDoes::Lose,
            wire: Wire::H2,
            content_type: "application/grpc",
            body: HI_FRAME,
            status: 200,
            grpc_status: Some("13"),
            connect_code: None,
            message: Some("internal error"),
            reply: Some(b""),
        },
        Case {
            name: "an undecodable reply is internal under json",
            php: PhpDoes::ReplyFf,
            wire: Wire::Http1,
            content_type: "application/json",
            body: br#"{"text":"hi"}"#,
            status: 500,
            grpc_status: None,
            connect_code: Some("internal"),
            message: Some("internal error"),
            reply: None,
        },
        Case {
            name: "an undecodable reply passes through under proto",
            php: PhpDoes::ReplyFf,
            wire: Wire::Http1,
            content_type: "application/proto",
            body: HI,
            status: 200,
            grpc_status: None,
            connect_code: None,
            message: None,
            reply: Some(&[0xff]),
        },
        Case {
            name: "malformed json never reaches php",
            php: PhpDoes::Unreached,
            wire: Wire::Http1,
            content_type: "application/json",
            body: b"{",
            status: 400,
            grpc_status: None,
            connect_code: Some("invalid_argument"),
            message: None,
            reply: None,
        },
    ];

    let srv = Spawn::grpc(wire_worker()).json_log().spawn();
    for case in cases {
        let before = calls(&srv).len();
        let mut conn = Conn::open(&listen(&srv), case.wire).await.expect("connect");
        let mut headers = vec![("content-type", case.content_type), ("te", "trailers")];
        match case.php {
            PhpDoes::Lose => headers.push(("x-reply", "none")),
            PhpDoes::ReplyFf => headers.push(("x-reply", "ff")),
            PhpDoes::Unreached => {}
        }
        let got = conn
            .send(Method::POST, ECHO_PATH, &headers, case.body)
            .await
            .unwrap_or_else(|e| panic!("{}: {e:#}", case.name));

        assert_eq!(got.status, case.status, "{}: {got:?}", case.name);
        assert_eq!(
            got.grpc_status(),
            case.grpc_status,
            "{}: {got:?}",
            case.name
        );
        if let Some(code) = case.connect_code {
            let body: Value = serde_json::from_slice(&got.body)
                .unwrap_or_else(|e| panic!("{}: {e}: {got:?}", case.name));
            assert_eq!(body["code"], code, "{}: {body}", case.name);
            if let Some(message) = case.message {
                assert_eq!(body["message"], message, "{}: {body}", case.name);
            }
        } else if let Some(message) = case.message {
            assert_eq!(got.grpc_message(), Some(message), "{}: {got:?}", case.name);
        }
        if let Some(reply) = case.reply {
            assert_eq!(&got.body[..], reply, "{}", case.name);
        }
        let reached = calls(&srv).len() - before;
        let want = usize::from(!matches!(case.php, PhpDoes::Unreached));
        assert_eq!(reached, want, "{}: the calls that reached PHP", case.name);
    }
    stop(srv).await;
}

/// Sources: the contract gRPC README (`Call\Context`), PROTOCOL-HTTP2 (`grpc-timeout`) and the Connect protocol (`connect-timeout-ms`).
#[tokio::test]
async fn request_facts_reach_php() {
    struct Case {
        name: &'static str,
        wire: Wire,
        content_type: &'static str,
        timeout: (&'static str, &'static str),
        body: &'static [u8],
        /// The `Call\Context::$protocol` value.
        protocol: &'static str,
    }
    let cases = [
        Case {
            name: "grpc",
            wire: Wire::H2,
            content_type: "application/grpc",
            timeout: ("grpc-timeout", "60S"),
            body: HI_FRAME,
            protocol: "grpc",
        },
        Case {
            name: "grpc-web",
            wire: Wire::Http1,
            content_type: "application/grpc-web+proto",
            timeout: ("grpc-timeout", "60S"),
            body: HI_FRAME,
            protocol: "grpc-web",
        },
        Case {
            name: "connect",
            wire: Wire::Http1,
            content_type: "application/proto",
            timeout: ("connect-timeout-ms", "60000"),
            body: HI,
            protocol: "connect",
        },
    ];

    let srv = Spawn::grpc(wire_worker()).json_log().spawn();
    for (seen, case) in cases.iter().enumerate() {
        let mut conn = Conn::open(&listen(&srv), case.wire).await.expect("connect");
        let wall = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs_f64();
        let headers = [
            ("content-type", case.content_type),
            ("te", "trailers"),
            ("x-a", "1"),
            case.timeout,
        ];
        let got = conn
            .send(Method::POST, ECHO_PATH, &headers, case.body)
            .await
            .unwrap_or_else(|e| panic!("{}: {e:#}", case.name));
        assert_eq!(got.status, 200, "{}: {got:?}", case.name);

        let calls = calls(&srv);
        assert_eq!(calls.len(), seen + 1, "{}: the call reached PHP", case.name);
        let call = &calls[seen];
        assert_eq!(
            call["method"], "rapira.test.v1.EchoService/Echo",
            "{}",
            case.name
        );
        assert_eq!(call["protocol"], case.protocol, "{}", case.name);
        assert_eq!(
            call["metadata"]["x-a"],
            json!(["1"]),
            "{}: {call}",
            case.name
        );
        let deadline = call["deadline"].as_f64();
        assert!(
            deadline.is_some_and(|d| d > wall + 50.0 && d < wall + 70.0),
            "{}: deadline {deadline:?} for a 60 s timeout at {wall}",
            case.name
        );
        assert_eq!(call["remote"], logged_remote(&conn.peer), "{}", case.name);
    }
    stop(srv).await;
}

/// Sources: the contract gRPC README (the server enforces `Call\Context::$deadline`) and PROTOCOL-HTTP2 `grpc-timeout` (`200m` is 200 ms, status 4 is DEADLINE_EXCEEDED).
#[tokio::test]
async fn a_passed_deadline_drops_the_call() {
    let srv = Spawn::grpc(fixture("grpc/unary-worker.php"))
        .json_log()
        .spawn();
    let mut conn = Conn::open(&listen(&srv), Wire::H2).await.expect("connect");
    // PHP holds the call until the plugin drops it.
    let got = conn
        .grpc(ECHO_PATH, &[("grpc-timeout", "200m")], &envelope(b"cancel"))
        .await
        .unwrap();

    assert_eq!(got.grpc_status(), Some("4"), "{got:?}");
    let cancel: Value =
        serde_json::from_str(&server_log::wait_app_record(&srv.log_file(), "cancel")).unwrap();
    assert_eq!(
        cancel["cancelled"], true,
        "the call must be dropped: {cancel}"
    );
    stop(srv).await;
}

/// Configured deadlines stop requests with stalled bodies. Connect returns HTTP 504; gRPC returns status 4.
#[tokio::test]
async fn configured_timeouts_bound_a_stalled_request() {
    /// An h2 request body that sends no frame and does not end.
    struct Stalled;
    impl Body for Stalled {
        type Data = Bytes;
        type Error = Infallible;
        fn poll_frame(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
        ) -> Poll<Option<Result<Frame<Bytes>, Infallible>>> {
            Poll::Pending
        }
    }
    struct Case {
        name: &'static str,
        /// Http1 sends a Connect request, H2 a gRPC request.
        wire: Wire,
        /// The client timeout field.
        timeout: Option<(&'static str, &'static str)>,
    }
    let cases = [
        Case {
            name: "connect, no client timeout",
            wire: Wire::Http1,
            timeout: None,
        },
        Case {
            name: "connect, client timeout above max_timeout_secs",
            wire: Wire::Http1,
            timeout: Some(("connect-timeout-ms", "60000")),
        },
        Case {
            name: "grpc, no client timeout",
            wire: Wire::H2,
            timeout: None,
        },
        Case {
            name: "grpc, client timeout above max_timeout_secs",
            wire: Wire::H2,
            timeout: Some(("grpc-timeout", "60S")),
        },
    ];

    let srv = Spawn::grpc(wire_worker())
        .grpc_extra("default_timeout_secs = 1\nmax_timeout_secs = 1")
        .json_log()
        .spawn();
    let addr = listen(&srv);
    for case in &cases {
        let started = Instant::now();
        match case.wire {
            Wire::Http1 => {
                let field = case
                    .timeout
                    .map_or(String::new(), |(k, v)| format!("{k}: {v}\r\n"));
                let request = format!(
                    "POST {ECHO_PATH} HTTP/1.1\r\nHost: e2e\r\nConnection: close\r\nContent-Type: application/proto\r\nContent-Length: 10\r\n{field}\r\n"
                );
                let (status, body) = http_raw(addr, request.as_bytes(), Duration::from_secs(10))
                    .unwrap_or_else(|e| panic!("{}: {e}", case.name));
                assert_eq!(status, 504, "{}", case.name);
                let body = String::from_utf8_lossy(&body);
                assert!(
                    body.contains(r#""code":"deadline_exceeded""#),
                    "{}: {body}",
                    case.name
                );
            }
            Wire::H2 => {
                let tcp = tokio::net::TcpStream::connect(addr).await.expect("connect");
                let (mut send, conn) =
                    hyper::client::conn::http2::handshake(TokioExecutor::new(), TokioIo::new(tcp))
                        .await
                        .expect("h2 handshake");
                tokio::spawn(conn);
                let mut req = http::Request::post(format!("http://localhost{ECHO_PATH}"))
                    .header("content-type", "application/grpc")
                    .header("te", "trailers");
                if let Some((k, v)) = case.timeout {
                    req = req.header(k, v);
                }
                let req = req.body(Stalled).expect("request");
                let got = tokio::time::timeout(Duration::from_secs(10), async {
                    let (parts, body) = send.send_request(req).await?.into_parts();
                    let collected = body.collect().await?;
                    anyhow::Ok(Response {
                        status: parts.status.as_u16(),
                        headers: parts.headers,
                        trailers: collected.trailers().cloned().unwrap_or_default(),
                        body: collected.to_bytes(),
                    })
                })
                .await
                .unwrap_or_else(|_| panic!("{}: no reply in 10 s", case.name))
                .unwrap_or_else(|e| panic!("{}: {e}", case.name));
                assert_eq!(got.grpc_status(), Some("4"), "{}: {got:?}", case.name);
            }
        }
        let elapsed = started.elapsed();
        // The deadline is 1 s. 5 s leaves time for a slow CI runner.
        assert!(
            (Duration::from_millis(900)..Duration::from_secs(5)).contains(&elapsed),
            "{}: took {elapsed:?}",
            case.name
        );
    }
    assert_eq!(calls(&srv), Vec::<Value>::new(), "no call reaches PHP");
    stop(srv).await;
}

/// Idle body reads use keepalive_timeout_secs. DEADLINE_EXCEEDED maps to HTTP 504: https://connectrpc.com/docs/protocol/#error-codes.
#[tokio::test]
async fn idle_rpc_bodies_timeout_without_a_short_call_deadline() {
    struct Case {
        name: &'static str,
        timeout: Option<&'static str>,
    }
    let cases = [
        Case {
            name: "no call deadline",
            timeout: None,
        },
        Case {
            name: "long client deadline",
            timeout: Some("connect-timeout-ms: 60000\r\n"),
        },
    ];
    let srv = Spawn::grpc(wire_worker())
        .grpc_extra("keepalive_timeout_secs = 1")
        .json_log()
        .spawn();
    let addr = listen(&srv);
    for case in cases {
        let timeout = case.timeout.unwrap_or_default();
        let request = format!(
            "POST {ECHO_PATH} HTTP/1.1\r\nHost: e2e\r\nConnection: close\r\nContent-Type: application/proto\r\nContent-Length: 10\r\n{timeout}\r\nx"
        );
        let started = Instant::now();
        let (status, body) = http_raw(addr, request.as_bytes(), Duration::from_secs(5))
            .unwrap_or_else(|error| {
                panic!(
                    "{}: idle body did not return a response: {error}",
                    case.name
                )
            });
        assert_eq!(status, 504, "{}", case.name);
        let body = String::from_utf8(body).expect("Connect error");
        assert!(
            body.contains(r#""code":"deadline_exceeded""#),
            "{}: {body}",
            case.name
        );
        assert!(
            body.contains(r#""message":"request body idle timeout""#),
            "{}: {body}",
            case.name
        );
        let elapsed = started.elapsed();
        assert!(
            (Duration::from_millis(900)..Duration::from_secs(5)).contains(&elapsed),
            "{}: took {elapsed:?}",
            case.name
        );
    }
    assert_eq!(
        calls(&srv),
        Vec::<Value>::new(),
        "no incomplete call reaches PHP"
    );
    stop(srv).await;
}

#[tokio::test]
async fn completed_rpc_bodies_do_not_limit_php_execution() {
    struct Case {
        name: &'static str,
        wire: Wire,
    }
    let cases = [
        Case {
            name: "Connect over HTTP/1.1",
            wire: Wire::Http1,
        },
        Case {
            name: "Connect over HTTP/2",
            wire: Wire::H2,
        },
    ];
    for case in cases {
        let srv = Spawn::grpc(fixture("grpc/unary-worker.php"))
            .grpc_extra("keepalive_timeout_secs = 1")
            .json_log()
            .spawn();
        let marker = srv.dir.join("marker");
        let message = format!("hold:{}", marker.display());
        let mut conn = Conn::open(&listen(&srv), case.wire).await.expect("connect");
        let client = tokio::spawn(async move {
            conn.send(
                Method::POST,
                ECHO_PATH,
                &[("content-type", "application/proto")],
                message.as_bytes(),
            )
            .await
        });
        let log = srv.log_file();
        tokio::task::spawn_blocking(move || server_log::wait_app_record(&log, "held"))
            .await
            .expect("the complete body reached PHP");
        tokio::time::sleep(Duration::from_millis(1500)).await;
        std::fs::write(&marker, b"go").expect("release PHP");
        let got = tokio::time::timeout(Duration::from_secs(5), client)
            .await
            .expect("PHP reply within 5 seconds")
            .expect("client task")
            .expect("response");
        assert_eq!(got.status, 200, "{}: {got:?}", case.name);
        assert_eq!(got.body.as_ref(), b"held", "{}", case.name);
        stop(srv).await;
    }
}

/// Sources: `grpc/health/v1/health.proto` (`HealthCheckRequest.service` and `HealthCheckResponse.status` are field 1, SERVING is 1), `grpc/reflection/v1/reflection.proto` (`list_services` is field 7), and PROTOCOL-HTTP2 (12 for an unknown method).
#[tokio::test]
async fn health_and_reflection_answer_from_the_plugin() {
    struct Case {
        name: &'static str,
        reflection: bool,
        path: &'static str,
        message: &'static [u8],
        grpc_status: &'static str,
        /// Bytes that the response body contains.
        reply: &'static [u8],
    }
    const REFLECTION: &str = "/grpc.reflection.v1.ServerReflection/ServerReflectionInfo";
    let cases = [
        Case {
            name: "health check, whole server",
            reflection: false,
            path: "/grpc.health.v1.Health/Check",
            message: b"",
            grpc_status: "0",
            reply: b"\x00\x00\x00\x00\x02\x08\x01",
        },
        Case {
            name: "health check, configured service",
            reflection: false,
            path: "/grpc.health.v1.Health/Check",
            message: b"\x0a\x1arapira.test.v1.EchoService",
            grpc_status: "0",
            reply: b"\x00\x00\x00\x00\x02\x08\x01",
        },
        Case {
            name: "reflection off",
            reflection: false,
            path: REFLECTION,
            message: b"\x3a\x00",
            grpc_status: "12",
            reply: b"",
        },
        Case {
            name: "reflection on",
            reflection: true,
            path: REFLECTION,
            message: b"\x3a\x00",
            grpc_status: "0",
            reply: b"rapira.test.v1.EchoService",
        },
    ];

    let plain = Spawn::grpc(wire_worker()).json_log().spawn();
    let reflecting = Spawn::grpc(wire_worker())
        .grpc_extra("reflection = true")
        .json_log()
        .spawn();
    for case in cases {
        let srv = if case.reflection { &reflecting } else { &plain };
        let mut conn = Conn::open(&listen(srv), Wire::H2).await.expect("connect");
        let got = conn
            .grpc(case.path, &[], &envelope(case.message))
            .await
            .unwrap_or_else(|e| panic!("{}: {e:#}", case.name));
        assert_eq!(
            got.grpc_status(),
            Some(case.grpc_status),
            "{}: {got:?}",
            case.name
        );
        assert!(
            case.reply.is_empty() || got.body.windows(case.reply.len()).any(|w| w == case.reply),
            "{}: {got:?}",
            case.name
        );
    }
    assert!(
        calls(&plain).is_empty() && calls(&reflecting).is_empty(),
        "the plugin answers without PHP"
    );
    stop(plain).await;
    stop(reflecting).await;
}

/// A graceful stop ends the plugin with no error, and the worker drops every intake clone.
#[tokio::test]
async fn shutdown_joins_the_server_and_drops_every_intake_clone() {
    let srv = Spawn::grpc(wire_worker()).json_log().spawn();
    let srv = stop(srv).await;
    assert_eq!(plugin_error(&srv), None);

    // The last intake clone goes with the worker's sink, and receive() then throws ClosedException.
    assert!(
        !server_log::app_contexts(&srv.log_file(), "closed").is_empty(),
        "every intake clone is gone"
    );
}

/// The drain grace is `[supervisor] process_control_timeout_secs` less the smaller of 5 s and half of it: 4 s gives 2 s, and 1 s gives 500 ms.
#[tokio::test]
async fn drain_waits_for_calls_within_the_grace() {
    struct Case {
        name: &'static str,
        /// PHP answers the call this long after the stop. None: PHP holds the call past the grace.
        answer_after: Option<Duration>,
        process_control_timeout_secs: u64,
        grpc_status: Option<&'static str>,
        /// A text of the plugin error. None: the plugin stops clean.
        shutdown: Option<&'static str>,
    }
    let cases = [
        Case {
            name: "a call that ends inside the grace completes",
            answer_after: Some(Duration::from_millis(100)),
            process_control_timeout_secs: 4,
            grpc_status: Some("0"),
            shutdown: None,
        },
        Case {
            name: "a call past the grace is cut",
            answer_after: None,
            process_control_timeout_secs: 1,
            grpc_status: None,
            shutdown: Some("grpc drain timed out"),
        },
    ];
    for case in cases {
        let srv = Spawn::grpc(fixture("grpc/unary-worker.php"))
            .toml(&format!(
                "[supervisor]\nprocess_control_timeout_secs = {}",
                case.process_control_timeout_secs
            ))
            .json_log()
            .spawn();
        let marker = srv.dir.join("marker");
        // `hold:` answers once the marker exists; `cancel` holds the call until the plugin drops it.
        let (message, reached) = match case.answer_after {
            Some(_) => (format!("hold:{}", marker.display()), "held"),
            None => ("cancel".to_owned(), "got"),
        };
        let mut conn = Conn::open(&listen(&srv), Wire::H2).await.expect("connect");
        let client = tokio::spawn(async move {
            conn.grpc(ECHO_PATH, &[], &envelope(message.as_bytes()))
                .await
        });
        let log = srv.log_file();
        tokio::task::spawn_blocking(move || server_log::wait_app_record(&log, reached))
            .await
            .expect("the call reached PHP");

        send_ctrl_break(srv.pid());
        if let Some(delay) = case.answer_after {
            tokio::time::sleep(delay).await;
            std::fs::write(&marker, b"go").unwrap();
        }
        let expected = if case.shutdown.is_some() { 1 } else { 0 };
        let srv = tokio::task::spawn_blocking(move || {
            let mut srv = srv;
            let status = srv.wait_exit(super::harness::STOP_BUDGET);
            super::harness::assert_exit_code(status, expected, &srv);
            srv
        })
        .await
        .unwrap();
        let got = client.await.unwrap();
        assert_eq!(
            got.as_ref().ok().and_then(|r| r.grpc_status()),
            case.grpc_status,
            "{}: {got:?}",
            case.name
        );
        let error = plugin_error(&srv);
        match case.shutdown {
            None => assert_eq!(error, None, "{}", case.name),
            Some(text) => assert!(
                error.as_ref().is_some_and(|e| e.contains(text)),
                "{}: {error:?}",
                case.name
            ),
        }
    }
}

#[tokio::test]
async fn drain_reports_a_cut_response_body() {
    struct Case {
        name: &'static str,
        wire: Wire,
        headers: Fields,
        request: &'static [u8],
    }
    let cases = [
        Case {
            name: "unread Connect body over HTTP/1.1",
            wire: Wire::Http1,
            headers: &[
                ("content-type", "application/proto"),
                ("connect-protocol-version", "1"),
                ("x-reply", "large-text"),
            ],
            request: HI,
        },
        Case {
            name: "unread gRPC body over HTTP/2",
            wire: Wire::H2,
            headers: &[
                ("content-type", "application/grpc"),
                ("te", "trailers"),
                ("x-reply", "large-text"),
            ],
            request: HI_FRAME,
        },
    ];
    for case in cases {
        let srv = Spawn::grpc(wire_worker())
            .toml("[supervisor]\nprocess_control_timeout_secs = 4")
            .json_log()
            .spawn();
        let mut conn = Conn::open(&listen(&srv), case.wire).await.expect("connect");
        let response = conn
            .request(Method::POST, ECHO_PATH, case.headers, case.request)
            .await
            .unwrap_or_else(|e| panic!("{}: {e:#}", case.name));
        assert_eq!(response.status(), 200, "{}", case.name);

        // Hold the body unread until shutdown completes.
        send_ctrl_break(srv.pid());
        let (srv, status) = tokio::task::spawn_blocking(move || {
            let mut srv = srv;
            let status = srv.wait_exit(super::harness::STOP_BUDGET);
            (srv, status)
        })
        .await
        .unwrap();
        let error = plugin_error(&srv);
        if matches!(case.wire, Wire::Http1) && status.is_some_and(|s| s.success()) {
            let body =
                tokio::time::timeout(Duration::from_secs(10), response.into_body().collect())
                    .await
                    .expect("response after shutdown")
                    .expect("a clean exit requires a complete response")
                    .to_bytes();
            assert_eq!(body.len(), 32 * 1024 * 1024 + 5, "{}", case.name);
            assert!(error.is_none(), "{}: {error:?}", case.name);
        } else {
            super::harness::assert_exit_code(status, 1, &srv);
            assert!(error.is_some(), "{}: missing drain error", case.name);
        }
    }
}

/// Source: the `StaticChecker` docs: `shutdown` only sets NOT_SERVING, and a `Watch` stream ends when its service is removed.
#[tokio::test]
async fn drain_ends_health_watch_streams() {
    // `process_control_timeout_secs = 4` gives a drain grace of 2 s.
    let grace = Duration::from_secs(2);
    let srv = Spawn::grpc(wire_worker())
        .toml("[supervisor]\nprocess_control_timeout_secs = 4")
        .json_log()
        .spawn();
    let mut conn = Conn::open(&listen(&srv), Wire::H2).await.expect("connect");
    let watch = conn
        .request(
            Method::POST,
            "/grpc.health.v1.Health/Watch",
            &[("content-type", "application/grpc"), ("te", "trailers")],
            &envelope(b""),
        )
        .await
        .unwrap();
    let mut body = watch.into_body();
    let first = body.frame().await.unwrap().unwrap().into_data().unwrap();
    assert_eq!(&first[..], b"\x00\x00\x00\x00\x02\x08\x01", "SERVING first");

    let started = Instant::now();
    send_ctrl_break(srv.pid());
    let (srv, rest) = tokio::join!(exited(srv), body.collect());
    assert_eq!(plugin_error(&srv), None);
    assert!(
        started.elapsed() < grace / 2,
        "took {:?}",
        started.elapsed()
    );
    let trailers = rest.unwrap().trailers().cloned().unwrap_or_default();
    assert_eq!(
        trailers.get("grpc-status").map(|v| v.as_bytes()),
        Some(&b"0"[..]),
        "{trailers:?}"
    );
}

enum Next {
    Frame { kind: u8, flags: u8 },
    Closed,
}

/// Reads one HTTP/2 frame. A read past the socket read timeout fails the test. https://www.rfc-editor.org/rfc/rfc9113#section-4.1
fn next_frame(stream: &mut TcpStream) -> Next {
    let mut head = [0u8; 9];
    if let Err(e) = stream.read_exact(&mut head) {
        return match e.kind() {
            io::ErrorKind::UnexpectedEof | io::ErrorKind::ConnectionReset => Next::Closed,
            _ => panic!("no frame within the read timeout: {e}"),
        };
    }
    let len = u32::from_be_bytes([0, head[0], head[1], head[2]]) as usize;
    let mut payload = vec![0u8; len];
    stream.read_exact(&mut payload).expect("the frame payload");
    Next::Frame {
        kind: head[3],
        flags: head[4],
    }
}

/// A silent peer gets a PING after `keepalive_interval_secs` without a read, and the listener closes the connection when the PING has no ACK within `keepalive_timeout_secs`. With 1 s each, both events come within the 4 s read timeout; the 10 s defaults miss it. https://www.rfc-editor.org/rfc/rfc9113#section-6.7
#[test]
fn keepalive_pings_a_silent_peer_and_closes_it() {
    const SETTINGS: u8 = 0x4;
    const PING: u8 = 0x6;
    const ACK: u8 = 0x1;
    let srv = Spawn::grpc(wire_worker())
        .grpc_extra("keepalive_interval_secs = 1\nkeepalive_timeout_secs = 1")
        .spawn();
    let addr = listen(&srv);
    let mut stream = TcpStream::connect(addr).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(4)))
        .expect("read timeout");
    // The client connection preface, then an empty SETTINGS frame. https://www.rfc-editor.org/rfc/rfc9113#section-3.4
    stream
        .write_all(b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n\0\0\0\x04\0\0\0\0\0")
        .expect("write the preface");
    loop {
        match next_frame(&mut stream) {
            Next::Frame {
                kind: SETTINGS,
                flags,
            } if flags & ACK == 0 => stream
                .write_all(b"\0\0\0\x04\x01\0\0\0\0")
                .expect("acknowledge the server SETTINGS"),
            Next::Frame { kind: PING, flags } if flags & ACK == 0 => break,
            Next::Frame { .. } => {}
            Next::Closed => panic!("the listener closed the connection before a PING"),
        }
    }
    // Only a GOAWAY may come before the close.
    while let Next::Frame { kind, .. } = next_frame(&mut stream) {
        assert_eq!(
            kind, 0x7,
            "a frame other than GOAWAY after the unanswered PING"
        );
    }
}

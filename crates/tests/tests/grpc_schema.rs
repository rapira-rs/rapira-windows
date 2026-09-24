//! The descriptor set behind a gRPC pool: what `Schema::load` accepts, and the JSON transcoding that Connect clients get from it.

use std::path::{Path, PathBuf};

use bytes::Bytes;
use http::Method;
use rapira_grpc::{MethodInfo, Schema, ServiceInfo};
use tests::grpc::{
    Answer, Conn, ECHO_PATH, ECHO_SERVICE, FakePhp, HI, Wire, config, scratch_dir, start, tcp,
};

fn load(path: &Path, services: &[&str]) -> anyhow::Result<Schema> {
    let services: Vec<String> = services.iter().map(|&s| s.to_owned()).collect();
    Schema::load(path, &services)
}

/// The listing keeps streaming methods, in echo.proto order.
#[test]
fn services_report_every_configured_method() {
    let method = |name: &str, server_streaming| MethodInfo {
        name: name.to_owned(),
        input_type: "rapira.test.v1.EchoRequest".to_owned(),
        output_type: "rapira.test.v1.EchoResponse".to_owned(),
        client_streaming: false,
        server_streaming,
    };
    let want = [ServiceInfo {
        name: ECHO_SERVICE.to_owned(),
        methods: vec![
            method("Echo", false),
            method("Get", false),
            method("Watch", true),
        ],
    }];
    let schema = load(&tests::echo_descriptor_set(), &[ECHO_SERVICE]).expect("echo.binpb loads");
    assert_eq!(schema.services(), want);
}

#[test]
fn load_rejects_a_set_it_cannot_serve() {
    struct Case {
        name: &'static str,
        path: PathBuf,
        service: &'static str,
        error: &'static str,
    }
    let dir = scratch_dir("schema");
    let not_a_set = dir.join("not-a-set.binpb");
    std::fs::write(&not_a_set, [0xff, 0xff]).unwrap();
    let cases = [
        Case {
            name: "unknown service",
            path: tests::echo_descriptor_set(),
            service: "rapira.test.v1.Missing",
            error: "grpc.services entry `rapira.test.v1.Missing` is not in",
        },
        Case {
            name: "missing file",
            path: PathBuf::from("/nonexistent.binpb"),
            service: ECHO_SERVICE,
            error: "reading grpc.descriptor_set",
        },
        Case {
            name: "not a set",
            path: not_a_set,
            service: ECHO_SERVICE,
            error: "--include_imports",
        },
        Case {
            name: "set without its imports",
            path: tests::fixture("grpc/echo-no-imports.binpb"),
            service: ECHO_SERVICE,
            error: "--include_imports",
        },
    ];
    for case in cases {
        let Err(err) = load(&case.path, &[case.service]) else {
            panic!("{}: the set loaded", case.name);
        };
        let err = err.to_string();
        assert!(err.contains(case.error), "{}: {err}", case.name);
    }
    let _ = std::fs::remove_dir_all(dir);
}

/// buffa resolves a name with a leading dot to the same service.
#[test]
fn load_rejects_a_service_listed_twice() {
    struct Case {
        name: &'static str,
        services: [&'static str; 2],
    }
    let cases = [
        Case {
            name: "exact duplicate",
            services: [ECHO_SERVICE, ECHO_SERVICE],
        },
        Case {
            name: "leading-dot alias",
            services: [ECHO_SERVICE, ".rapira.test.v1.EchoService"],
        },
    ];
    for case in cases {
        let Err(err) = load(&tests::echo_descriptor_set(), &case.services) else {
            panic!("{}: the set loaded", case.name);
        };
        let err = err.to_string();
        assert!(
            err.contains("grpc.services lists `rapira.test.v1.EchoService` twice"),
            "{}: {err}",
            case.name
        );
    }
}

/// A Connect JSON call transcodes by the descriptor: the request into the bytes PHP gets, the reply out of the bytes PHP returns.
///
/// Sources: the protobuf encoding (field 1 string "hi" is 0a 02 68 69, field 2 Timestamp{seconds: 1} is 12 02 08 01, field 3 packed repeated int32 is 1a, the varint length, then one varint per element), proto3 JSON (a repeated int32 is an array of numbers, defaults are omitted, unknown fields are ignored) and the Connect protocol (400 for a request that does not decode, 500 for a reply that does not).
#[tokio::test]
async fn json_transcodes_by_descriptor() {
    // buffa charges the size of its Value type, at least 64 bytes, per element against a 32 MiB budget for untrusted input, so at most 524,288 elements fit. The reply is the application's, so the budget does not apply to it.
    const IDS: usize = 600_000;
    let mut many_ids = vec![0x1a, 0xc0, 0xcf, 0x24];
    many_ids.resize(many_ids.len() + IDS, 0x01);
    let many_ids_json = format!(r#"{{"ids":[{}1]}}"#, "1,".repeat(IDS - 1));
    struct Case<'a> {
        name: &'static str,
        answer: Answer,
        request: &'a [u8],
        status: u16,
        /// The bytes PHP gets. None: PHP never sees the call.
        sent: Option<&'a [u8]>,
        /// None: not checked.
        reply: Option<&'a [u8]>,
    }
    let cases = [
        Case {
            name: "json to proto and back",
            answer: Answer::Echo,
            request: br#"{"text":"hi"}"#,
            status: 200,
            sent: Some(HI),
            reply: Some(br#"{"text":"hi"}"#),
        },
        Case {
            name: "well-known type both ways",
            answer: Answer::Echo,
            request: br#"{"text":"hi","at":"1970-01-01T00:00:01Z"}"#,
            status: 200,
            sent: Some(&[0x0a, 0x02, 0x68, 0x69, 0x12, 0x02, 0x08, 0x01]),
            reply: Some(br#"{"text":"hi","at":"1970-01-01T00:00:01Z"}"#),
        },
        Case {
            name: "unknown field ignored",
            answer: Answer::Echo,
            request: br#"{"text":"hi","x":1}"#,
            status: 200,
            sent: Some(HI),
            reply: Some(br#"{"text":"hi"}"#),
        },
        Case {
            name: "defaults omitted",
            answer: Answer::Echo,
            request: b"{}",
            status: 200,
            sent: Some(b""),
            reply: Some(b"{}"),
        },
        Case {
            name: "wrong json type",
            answer: Answer::Echo,
            request: br#"{"text":1}"#,
            status: 400,
            sent: None,
            reply: None,
        },
        Case {
            name: "not json",
            answer: Answer::Echo,
            request: b"not json",
            status: 400,
            sent: None,
            reply: None,
        },
        Case {
            name: "invalid utf-8",
            answer: Answer::Echo,
            request: b"{\"text\":\"\xff\"}",
            status: 400,
            sent: None,
            reply: None,
        },
        Case {
            name: "undecodable reply",
            answer: Answer::Reply(Bytes::from_static(&[0xff])),
            request: br#"{"text":"hi"}"#,
            status: 500,
            sent: Some(HI),
            reply: None,
        },
        Case {
            name: "reply above the untrusted-input element budget",
            answer: Answer::Reply(Bytes::from(many_ids.clone())),
            request: br#"{"text":"hi"}"#,
            status: 200,
            sent: Some(HI),
            reply: Some(many_ids_json.as_bytes()),
        },
    ];

    let php = FakePhp::new(Answer::Echo);
    let mut running = start(config(tcp()), php.clone()).await;
    let mut conn = Conn::open(&running.listen, Wire::Http1)
        .await
        .expect("connect");
    for case in cases {
        php.answer(case.answer);
        let before = php.seen();
        let got = conn
            .send(
                Method::POST,
                ECHO_PATH,
                &[("content-type", "application/json")],
                case.request,
            )
            .await
            .unwrap_or_else(|e| panic!("{}: {e:#}", case.name));

        assert_eq!(got.status, case.status, "{}: {got:?}", case.name);
        assert_eq!(
            php.seen() - before,
            usize::from(case.sent.is_some()),
            "{}: the call reaching PHP",
            case.name
        );
        if let Some(sent) = case.sent {
            assert_eq!(
                php.last_message().as_deref(),
                Some(sent),
                "{}: the message PHP got",
                case.name
            );
        }
        if let Some(reply) = case.reply {
            assert!(
                &got.body[..] == reply,
                "{}: {}",
                case.name,
                String::from_utf8_lossy(&got.body[..got.body.len().min(200)])
            );
        }
    }
    running.shutdown().await.unwrap();
}

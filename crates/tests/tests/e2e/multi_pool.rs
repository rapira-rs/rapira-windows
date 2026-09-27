use std::collections::BTreeSet;
use std::time::Duration;

use http::Method;
use rapira_sapi::Mode;
use serde_json::Value;
use tests::fixture;
use tests::grpc::{Conn, ECHO_PATH, Wire};

use crate::harness::{Spawn, http_get, listen};

fn check(body: &[u8], plugin: &str, mode: &str, input: &str) -> String {
    let value: Value = serde_json::from_slice(body).expect("PHP JSON response");
    assert_eq!(value["plugin"], plugin, "{value}");
    assert_eq!(value["mode"], mode, "{value}");
    assert_eq!(value["script"], format!("{plugin}.php"), "{value}");
    assert_eq!(value["input"], input, "{value}");
    let token = value["token"].as_str().expect("bootstrap token");
    assert_eq!(value["directory"], format!("{plugin}-{token}"), "{value}");
    token.to_owned()
}

/// Both pools recycle under concurrent traffic. Each response must retain its mode, entrypoint, virtual directory, and request data.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn interpreter_state_is_local_to_each_plugin_and_generation() -> anyhow::Result<()> {
    for (mode, name) in [(Mode::Worker, "Worker"), (Mode::Dispatcher, "Dispatcher")] {
        let mut srv = Spawn::http(mode, fixture("multi_pool/http/http.php"))
            .with_grpc(fixture("multi_pool/grpc/grpc.php"))
            .processes(2, 2)
            .http_pool("max_requests = 2")
            .grpc_pool("max_requests = 2")
            .spawn();
        let mut http_clients = Vec::new();
        for producer in 0..4 {
            let addr = srv.addr;
            http_clients.push(std::thread::spawn(move || {
                let mut tokens = BTreeSet::new();
                for request in 0..12 {
                    let input = format!("/?producer={producer}&request={request}");
                    let (status, body) = http_get(addr, &input, Duration::from_secs(10)).unwrap();
                    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
                    tokens.insert(check(&body, "http", name, &input));
                }
                tokens
            }));
        }
        let mut grpc_clients = Vec::new();
        for producer in 0..4 {
            let addr = listen(&srv);
            grpc_clients.push(tokio::spawn(async move {
                let mut tokens = BTreeSet::new();
                let mut conn = Conn::open(&addr, Wire::Http1).await.unwrap();
                for request in 0..12 {
                    let input = format!("producer={producer}&request={request}");
                    let reply = conn
                        .send(
                            Method::POST,
                            ECHO_PATH,
                            &[
                                ("content-type", "application/proto"),
                                ("connect-protocol-version", "1"),
                            ],
                            input.as_bytes(),
                        )
                        .await
                        .unwrap();
                    assert_eq!(reply.status, 200, "{reply:?}");
                    tokens.insert(check(&reply.body, "grpc", "Dispatcher", &input));
                }
                tokens
            }));
        }
        let mut grpc_tokens = BTreeSet::new();
        for client in grpc_clients {
            grpc_tokens.extend(tokio::time::timeout(Duration::from_secs(30), client).await??);
        }
        let mut http_tokens = BTreeSet::new();
        for client in http_clients {
            http_tokens.extend(client.join().unwrap());
        }
        assert!(
            http_tokens.len() > 2 && grpc_tokens.len() > 2,
            "both pools must serve after recycling"
        );
        srv.stop();
    }
    Ok(())
}

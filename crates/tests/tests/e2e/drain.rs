use std::time::Duration;

use crate::console::send_ctrl_break;
use crate::harness::{Conn, Spawn, diagnostics, fixture_path, wait_log_contains};
use rapira_sapi::Mode;
use tests::fixture;

const READ: Duration = Duration::from_secs(10);

fn exchange(conn: &mut Conn) -> std::io::Result<(u16, Vec<(String, String)>)> {
    conn.send(b"GET / HTTP/1.1\r\nHost: e2e\r\n\r\n")?;
    let head = conn.read_head(READ)?;
    let length = head
        .1
        .iter()
        .find(|(name, _)| name == "content-length")
        .and_then(|(_, value)| value.parse().ok())
        .expect("buffered response length");
    conn.read_n(length, READ)?;
    Ok(head)
}

#[test]
fn shutdown_answers_requests_on_held_http_connections() -> anyhow::Result<()> {
    let mut srv = Spawn::http(Mode::Worker, fixture("shared/worker.php")).spawn();
    let mut connections = Vec::new();
    for _ in 0..4 {
        let mut conn = Conn::open(srv.addr, READ)?;
        assert_eq!(exchange(&mut conn)?.0, 200);
        connections.push(conn);
    }
    send_ctrl_break(srv.pid());
    assert!(
        wait_log_contains(&srv, "open connections", READ),
        "{}",
        diagnostics(&srv)
    );
    for conn in &mut connections {
        let (status, headers) = exchange(conn)?;
        assert_eq!(status, 200);
        assert!(
            headers
                .iter()
                .any(|(name, value)| name == "connection" && value == "close")
        );
    }
    assert!(
        srv.wait_exit(READ).is_some_and(|status| status.success()),
        "{}",
        diagnostics(&srv)
    );
    Ok(())
}

#[tokio::test]
async fn shutdown_answers_connect_calls_on_held_http1_connections() -> anyhow::Result<()> {
    use tests::grpc::{self, Wire};
    let mut srv = Spawn::grpc(fixture_path("grpc/echo-worker.php")).spawn();
    let mut connections = Vec::new();
    for _ in 0..4 {
        let mut conn = grpc::Conn::open(&srv.addr, Wire::Http1).await?;
        let response = conn
            .send(
                http::Method::POST,
                grpc::ECHO_PATH,
                &[
                    ("content-type", "application/proto"),
                    ("connect-protocol-version", "1"),
                ],
                grpc::HI,
            )
            .await?;
        assert_eq!(response.status, 200);
        connections.push(conn);
    }
    send_ctrl_break(srv.pid());
    assert!(
        wait_log_contains(&srv, "draining open connections", READ),
        "{}",
        diagnostics(&srv)
    );
    for conn in &mut connections {
        let response = tokio::time::timeout(
            READ,
            conn.send(
                http::Method::POST,
                grpc::ECHO_PATH,
                &[
                    ("content-type", "application/proto"),
                    ("connect-protocol-version", "1"),
                ],
                grpc::HI,
            ),
        )
        .await??;
        assert_eq!(response.status, 200);
        assert_eq!(
            response
                .headers
                .get(http::header::CONNECTION)
                .and_then(|value| value.to_str().ok()),
            Some("close")
        );
    }
    assert!(
        srv.wait_exit(READ).is_some_and(|status| status.success()),
        "{}",
        diagnostics(&srv)
    );
    Ok(())
}

use php_sys::{Mode, Rapira, grpc};
use std::time::{Duration, Instant};
use tests::{captured, drain, fixture, init_log_capture, php_lock, req};

#[test]
fn empty_receive_outcomes_restore_the_execution_budget() -> anyhow::Result<()> {
    struct Case {
        name: &'static str,
        grpc: bool,
        action: &'static str,
        outcome: &'static str,
    }
    let cases = [
        Case {
            name: "http_poll",
            grpc: false,
            action: "poll",
            outcome: "empty",
        },
        Case {
            name: "grpc_poll",
            grpc: true,
            action: "poll",
            outcome: "empty",
        },
        Case {
            name: "http_zero_timeout",
            grpc: false,
            action: "zero",
            outcome: "timeout",
        },
        Case {
            name: "grpc_zero_timeout",
            grpc: true,
            action: "zero",
            outcome: "timeout",
        },
        Case {
            name: "http_finite_timeout",
            grpc: false,
            action: "finite",
            outcome: "timeout",
        },
        Case {
            name: "grpc_finite_timeout",
            grpc: true,
            action: "finite",
            outcome: "timeout",
        },
        Case {
            name: "http_closed",
            grpc: false,
            action: "closed",
            outcome: "closed",
        },
        Case {
            name: "grpc_closed",
            grpc: true,
            action: "closed",
            outcome: "closed",
        },
    ];
    let _guard = php_lock();
    init_log_capture();
    let rt = tokio::runtime::Builder::new_current_thread().build()?;
    for case in cases {
        captured().clear();
        let entrypoint = fixture("timeout_tests/receive-worker.php");
        let mode = if case.grpc {
            Mode::GrpcDispatcher {
                entrypoint,
                services: Vec::new(),
            }
        } else {
            Mode::Dispatcher(entrypoint)
        };
        let rapira = Rapira::start(mode)?;
        let handle = rapira.handle();
        if case.grpc {
            let reply = rt.block_on(handle.handle_grpc(grpc::Request {
                method: "example.Timer/Probe".into(),
                message: bytes::Bytes::from_static(case.action.as_bytes()),
                metadata: Vec::new(),
                remote: php_sys::types::Addr::Inet(([127, 0, 0, 1], 1234).into()),
                tls: None,
                received_at: 0.0,
                deadline: None,
                expires_at: None,
            }))?;
            assert_eq!(reply.result.unwrap().as_ref(), b"ready", "{}", case.name);
        } else {
            let (status, body) = drain(handle.handle_blocking(req(
                &format!("/{}", case.action),
                "timeout_tests/receive-worker.php",
            ))?);
            assert_eq!((status, body.as_str()), (200, "ready"), "{}", case.name);
        }
        if case.action != "closed" {
            let end = Instant::now() + Duration::from_secs(10);
            while !captured()
                .iter()
                .any(|r| r.target == "app" && r.message == "receive-timer-finished")
            {
                assert!(Instant::now() < end, "{} did not finish", case.name);
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        drop(handle);
        assert!(rapira.shutdown(), "{} did not stop", case.name);
        let records = captured();
        let outcome = records
            .iter()
            .find(|r| r.target == "app" && r.message == "receive-timer-outcome")
            .expect("receive outcome");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&outcome.context)?,
            serde_json::json!({"outcome": case.outcome}),
            "{}",
            case.name
        );
        assert!(
            records.iter().any(|r| r.target == "php"
                && r.level == tracing::Level::ERROR
                && r.message.contains("Maximum execution time")),
            "{} did not report an execution timeout: {records:#?}",
            case.name
        );
        assert!(
            !records
                .iter()
                .any(|r| r.target == "app" && r.message == "receive-timer-survived"),
            "{} exceeded the execution budget",
            case.name
        );
    }
    Ok(())
}

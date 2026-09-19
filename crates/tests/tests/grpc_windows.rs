use php_sys::{Mode, PoolConfig, PoolHooks, Rapira, grpc};
use tests::{drain_async, fixture, php_lock, req};

#[test]
fn protocol_threads_keep_their_modes_and_recycle_in_one_process() -> anyhow::Result<()> {
    let _guard = php_lock();
    let rapira = Rapira::start_pools(vec![
        PoolConfig {
            mode: Mode::Worker(fixture("mode/worker.php")),
            processes: 2,
            hooks: PoolHooks {
                max_requests: 1,
                ..Default::default()
            },
        },
        PoolConfig {
            mode: Mode::GrpcDispatcher {
                entrypoint: fixture("grpc/mixed.php"),
                services: Vec::new(),
            },
            processes: 2,
            hooks: PoolHooks {
                max_requests: 1,
                ..Default::default()
            },
        },
    ])?;
    let http = rapira.pool_handle(0);
    let grpc = rapira.pool_handle(1);
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()?
        .block_on(async {
            for _ in 0..12 {
                let http_call = async {
                    drain_async(http.handle(req("/", "mode/worker.php")).await.unwrap()).await
                };
                let grpc_call = grpc.handle_grpc(grpc::Request {
                    method: "example.Echo/Call".into(),
                    message: bytes::Bytes::new(),
                    metadata: Vec::new(),
                    remote: php_sys::types::Addr::Inet(([127, 0, 0, 1], 1234).into()),
                    tls: None,
                    received_at: 0.0,
                    deadline: None,
                    expires_at: None,
                });
                let (http_reply, grpc_reply) =
                    tokio::time::timeout(std::time::Duration::from_secs(10), async {
                        tokio::join!(http_call, grpc_call)
                    })
                    .await
                    .expect("both protocol queues must progress");
                assert_eq!(http_reply, (200, "Worker:case:same:unbacked".into()));
                assert_eq!(
                    grpc_reply.unwrap().result.unwrap().as_ref(),
                    format!("Dispatcher:grpc:{}", std::process::id()).as_bytes()
                );
            }
        });
    let snapshot = rapira.scoreboard();
    assert_eq!(snapshot.workers.len(), 4);
    assert!(snapshot.recycles >= 2);
    drop((http, grpc));
    assert!(rapira.shutdown(), "all interpreter threads must stop");
    Ok(())
}

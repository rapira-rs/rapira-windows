use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use anyhow::{Result, anyhow};
use bytes::Bytes;
use connectrpc::http;
use connectrpc::server::serve_connection;
use connectrpc::{
    Chain, CompressionRegistry, ConnectRpcBody, ConnectRpcService, ConnectionConfig,
    ConnectionInfo, DeadlinePolicy, GzipProvider, Router,
};
use connectrpc_health::StaticChecker;
use http_body_util::BodyExt as _;
use rapira_net::{Acceptor, Serve};
use rapira_sapi::Addr;
use rapira_sapi::plugin::Worker;
use rapira_sapi::work::Intake;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::watch;
use tower::ServiceExt as _;

use crate::dispatch::PhpDispatcher;
use crate::{Config, Prepared};

/// Everything the accept loop hands to a connection, and the drain that follows it.
struct Serving {
    service: ConnectRpcService<Chain<Router, PhpDispatcher>>,
    connection: ConnectionConfig,
    health: Arc<StaticChecker>,
    /// Each connection holds a receiver until it ends, so `closed()` resolves when the last connection is gone.
    shutdown: watch::Sender<bool>,
    inflight: Arc<AtomicUsize>,
    http2: Arc<AtomicUsize>,
}

struct InFlight(Arc<AtomicUsize>);

impl InFlight {
    fn start(count: &Arc<AtomicUsize>) -> Self {
        count.fetch_add(1, Relaxed);
        Self(Arc::clone(count))
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Relaxed);
    }
}

struct CountedBytes {
    data: Bytes,
    _counted: Arc<InFlight>,
}

impl AsRef<[u8]> for CountedBytes {
    fn as_ref(&self) -> &[u8] {
        &self.data
    }
}

impl Serving {
    fn start(intake: Intake, config: &Config, router: Router, health: Arc<StaticChecker>) -> Self {
        tracing::info!(target: "grpc", "listening on {}", config.listen);
        let mut deadlines = DeadlinePolicy::new();
        if let Some(timeout) = config.default_timeout {
            deadlines = deadlines.with_default_timeout(timeout);
        }
        if let Some(max) = config.max_timeout {
            deadlines = deadlines.with_max(max);
        }
        let dispatcher = PhpDispatcher {
            schema: Arc::clone(&config.schema),
            intake,
        };
        // The plugin routes come first, so a configured service cannot hide health or reflection.
        let mut service = ConnectRpcService::new(Chain(router, dispatcher))
            .with_deadline_policy(deadlines)
            // The default registry offers every codec that the build compiles, zstd included.
            .with_compression(CompressionRegistry::new().register(GzipProvider::default()));
        for interceptor in &config.interceptors {
            service = service.with_interceptor_arc(Arc::clone(interceptor));
        }
        Self {
            service,
            connection: ConnectionConfig::new()
                .with_http2_keepalive_interval(config.keepalive_interval)
                .with_http2_keepalive_timeout(config.keepalive_timeout),
            health,
            shutdown: watch::Sender::new(false),
            inflight: Arc::new(AtomicUsize::new(0)),
            http2: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn spawn<I>(&self, io: I, info: ConnectionInfo)
    where
        I: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let open = self.shutdown.subscribe();
        let mut stop = open.clone();
        let draining = open.clone();
        let http1 = Arc::new(AtomicBool::new(false));
        let (mark, close) = (Arc::clone(&http1), Arc::clone(&http1));
        let inflight = Arc::clone(&self.inflight);
        let http2 = Arc::new(OnceLock::new());
        let counted_http2 = Arc::clone(&http2);
        let http2_connections = Arc::clone(&self.http2);
        let service = self
            .service
            .clone()
            .map_request(move |req: http::Request<_>| {
                mark.store(req.version() < http::Version::HTTP_2, Relaxed);
                if req.version() == http::Version::HTTP_2 {
                    counted_http2.get_or_init(|| InFlight::start(&http2_connections));
                }
                req
            })
            .map_response(move |response| {
                let mut response = status_in_trailers(response);
                if close.load(Relaxed) && *draining.borrow() {
                    response.headers_mut().insert(
                        http::header::CONNECTION,
                        http::HeaderValue::from_static("close"),
                    );
                }
                response
            })
            .map_future(move |call| {
                let counted = Arc::new(InFlight::start(&inflight));
                async move {
                    call.await.map(|response| {
                        response.map(|body| {
                            body.map_frame(move |frame| {
                                // Transport buffers can retain data after the body is dropped.
                                frame.map_data(|data| {
                                    Bytes::from_owner(CountedBytes {
                                        data,
                                        _counted: Arc::clone(&counted),
                                    })
                                })
                            })
                        })
                    })
                }
            });
        let connection = serve_connection(io, info, service, self.connection.clone(), async move {
            let _ = stop.wait_for(|stop| *stop).await;
            if http1.load(Relaxed) {
                std::future::pending::<()>().await;
            }
        });
        tokio::spawn(async move {
            // The shutdown future drops its receiver when shutdown starts, and the calls in flight continue after that. `open` keeps `closed()` pending until the connection ends.
            let closed = connection.await;
            drop(http2);
            drop(open);
            tracing::debug!(target: "grpc", "connection closed: {:?}", closed.reason());
        });
    }

    /// Waits out the connections in flight. The acceptor is already gone.
    async fn drain(self, fatal: Option<anyhow::Error>, grace: Duration) -> Result<()> {
        // `StaticChecker::shutdown` only sets NOT_SERVING. A `Watch` stream ends only when its service is removed, and an open stream holds its connection until the grace ends.
        self.health.shutdown();
        for name in self.health.services() {
            self.health.remove_service(&name);
        }
        tracing::info!(target: "grpc", "draining open connections");
        self.shutdown.send_replace(true);
        let drained = tokio::time::timeout(grace, self.shutdown.closed())
            .await
            .is_ok();
        if let Some(e) = fatal {
            return Err(e);
        }
        let cut = self.inflight.load(Relaxed);
        let http2 = self.http2.load(Relaxed);
        if !drained && (cut > 0 || http2 > 0) {
            return Err(anyhow!(
                "grpc drain timed out after {grace:?}; {cut} call(s) and {http2} HTTP/2 connection(s) remain"
            ));
        }
        tracing::info!(target: "grpc", "drained cleanly; accept loop stopped");
        Ok(())
    }
}

impl Serve for Serving {
    fn spawn_tcp(&self, stream: tokio::net::TcpStream, peer: std::net::SocketAddr) {
        let mut info = ConnectionInfo::new().with_peer_addr(peer);
        info.extensions_mut().insert(Addr::Inet(peer));
        self.spawn(stream, info);
    }
}

/// A gRPC error carries its whole status in the trailers. connectrpc also copies the code and the message into the head, and a client that takes the status from the head, such as tonic, then drops the details and the trailers: https://github.com/connectrpc/connect-rust/issues/286. Only that copy puts these fields in a head, because PHP cannot set a `grpc-` field.
fn status_in_trailers(
    mut response: http::Response<ConnectRpcBody>,
) -> http::Response<ConnectRpcBody> {
    let headers = response.headers_mut();
    headers.remove("grpc-status");
    headers.remove("grpc-message");
    response
}

/// Runs the accept loop on the calling thread until the stop flag, then drains the connections.
pub(crate) fn serve(config: Config, prepared: Prepared, worker: Worker) -> Result<()> {
    let Worker {
        handle,
        intake,
        stop,
        drain_grace,
    } = worker;
    let acceptor = Acceptor::adopt(prepared.listener, stop, &handle)?;
    let serving = Serving::start(intake, &config, prepared.router, prepared.health);
    let fatal = acceptor.run(&handle, &serving);
    handle.block_on(serving.drain(fatal, drain_grace))
}

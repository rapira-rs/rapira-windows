use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, anyhow};
use hyper::body::Incoming;
use hyper::header::{CONTENT_TYPE, HeaderValue};
use hyper::server::conn::http1;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::{TokioIo, TokioTimer};
use hyper_util::server::graceful::GracefulShutdown;
use rapira_net::{Acceptor, PreparedListener, Serve};
use rapira_scoreboard::Pool;
use tokio::sync::watch;

use crate::config::Settings;
use crate::text::{self, Build};
use crate::{probes, stats};

pub struct Server {
    build: Build,
    prepared: PreparedListener,
    keepalive_timeout: Duration,
    metrics: bool,
    probes: bool,
}

impl Server {
    pub fn new(settings: Settings, build: Build) -> Result<Self> {
        let prepared = rapira_net::bind(&settings.listen)?;
        tracing::info!(target: "observability", "prepared listener on {}", prepared.addr());
        Ok(Self {
            build,
            prepared,
            keepalive_timeout: settings.keepalive_timeout,
            metrics: settings.metrics,
            probes: settings.probes,
        })
    }

    pub fn serve(
        self,
        pools: Vec<Arc<Pool>>,
        stop: watch::Receiver<bool>,
        drain_grace: Duration,
    ) -> Result<()> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .thread_name("rapira-obs-io")
            .build()
            .map_err(|e| anyhow!("building the observability runtime: {e}"))?;
        tracing::info!(target: "observability", "serving on {}", self.prepared.addr());
        let acceptor = Acceptor::adopt(self.prepared, stop, rt.handle())?;
        let mut builder = http1::Builder::new();
        builder
            .timer(TokioTimer::new())
            .header_read_timeout(self.keepalive_timeout);
        let serving = Serving {
            routes: Arc::new(Routes {
                pools,
                build: self.build,
                metrics: self.metrics,
                probes: self.probes,
            }),
            graceful: GracefulShutdown::new(),
            builder,
        };
        let fatal = acceptor.run(rt.handle(), &serving);
        let Serving { graceful, .. } = serving;
        let drained =
            rt.block_on(async { tokio::time::timeout(drain_grace, graceful.shutdown()).await });
        if drained.is_err() {
            tracing::warn!(target: "observability", "requests still in flight after {drain_grace:?}");
        }
        fatal.map_or(Ok(()), Err)
    }
}

struct Routes {
    pools: Vec<Arc<Pool>>,
    build: Build,
    metrics: bool,
    probes: bool,
}

struct Serving {
    routes: Arc<Routes>,
    graceful: GracefulShutdown,
    builder: http1::Builder,
}

impl Serve for Serving {
    fn spawn_tcp(&self, stream: tokio::net::TcpStream, _peer: std::net::SocketAddr) {
        let routes = self.routes.clone();
        let service = hyper::service::service_fn(move |req| {
            std::future::ready(Ok::<_, Infallible>(respond(&routes, &req)))
        });
        let conn = self
            .graceful
            .watch(self.builder.serve_connection(TokioIo::new(stream), service));
        tokio::spawn(async move {
            if let Err(e) = conn.await {
                tracing::debug!(target: "observability", "connection ended with error: {e}");
            }
        });
    }
}

const PROBE_CONTENT_TYPE: &str = "text/plain; charset=utf-8";

fn respond(routes: &Routes, req: &Request<Incoming>) -> Response<String> {
    match (req.method(), req.uri().path()) {
        (&Method::GET, "/metrics") if routes.metrics => reply(
            StatusCode::OK,
            text::CONTENT_TYPE,
            text::render(&stats::board_stats(&routes.pools), &routes.build),
        ),
        (&Method::GET, "/livez") if routes.probes => {
            reply(StatusCode::OK, PROBE_CONTENT_TYPE, "ok\n")
        }
        (&Method::GET, "/readyz") if routes.probes => {
            let unready = probes::unready(&routes.pools);
            if unready.is_empty() {
                reply(StatusCode::OK, PROBE_CONTENT_TYPE, "ok\n")
            } else {
                let body: String = unready
                    .iter()
                    .map(|name| format!("pool {name}: no ready worker\n"))
                    .collect();
                reply(StatusCode::SERVICE_UNAVAILABLE, PROBE_CONTENT_TYPE, body)
            }
        }
        _ => {
            let mut response = Response::new(String::new());
            *response.status_mut() = StatusCode::NOT_FOUND;
            response
        }
    }
}

fn reply(
    status: StatusCode,
    content_type: &'static str,
    body: impl Into<String>,
) -> Response<String> {
    let mut response = Response::new(body.into());
    *response.status_mut() = status;
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
    response
}

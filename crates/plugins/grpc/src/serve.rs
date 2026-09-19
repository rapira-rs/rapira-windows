use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use anyhow::anyhow;
use extension_api::{Addr, Php, PreparedListener, Result};
use hyper::server::conn::http2;
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use hyper_util::server::graceful::GracefulShutdown;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpListener;
use tokio::sync::watch;

use crate::Config;
use crate::handler::{RapiraService, Shared};

pub(crate) async fn serve(
    php: Php,
    config: Config,
    prepared: PreparedListener,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    let listen = prepared.addr().clone();
    let listener = TcpListener::from_std(prepared.into_listener())?;
    let shared = Arc::new(Shared::new(config, php)?);
    let graceful = GracefulShutdown::new();
    let mut builder = http2::Builder::new(TokioExecutor::new());
    builder.timer(TokioTimer::new());
    tracing::info!(target: "grpc", "listening on {listen:?}");
    let mut fatal = None;
    loop {
        tokio::select! {
            biased;
            _ = shutdown.wait_for(|stop| *stop) => break,
            result = accept_connection(&listener, &builder, &graceful, &shared) => match result {
                Ok(()) => {}
                Err(error) if is_fatal_accept(&error) => {
                    fatal = Some(anyhow!("listener failed: {error}"));
                    break;
                }
                Err(error) if matches!(error.kind(), std::io::ErrorKind::ConnectionAborted | std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::Interrupted) => {
                    tracing::debug!(target: "grpc", "accept skipped: {error}");
                }
                Err(error) => {
                    tracing::warn!(target: "grpc", "accept failed: {error}");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        }
    }
    drop(listener);
    let deadline = tokio::time::Instant::now() + shared.cfg.drain_grace;
    let timed_out = tokio::time::timeout_at(deadline, graceful.shutdown())
        .await
        .is_err();
    while shared.inflight.load(Ordering::Acquire) > 0 && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    if let Some(error) = fatal {
        return Err(error);
    }
    let stranded = shared.inflight.load(Ordering::Acquire);
    if timed_out || stranded > 0 {
        return Err(anyhow!(
            "grpc drain timed out after {:?} with {stranded} call(s) in flight",
            shared.cfg.drain_grace
        ));
    }
    tracing::info!(target: "grpc", "drained cleanly; accept loop stopped");
    Ok(())
}

fn is_fatal_accept(error: &std::io::Error) -> bool {
    use windows_sys::Win32::Networking::WinSock::{WSAEINVAL, WSAENOTSOCK, WSAEOPNOTSUPP};
    matches!(
        error.raw_os_error(),
        Some(WSAENOTSOCK | WSAEINVAL | WSAEOPNOTSUPP)
    )
}

async fn accept_connection(
    listener: &TcpListener,
    builder: &http2::Builder<TokioExecutor>,
    graceful: &GracefulShutdown,
    shared: &Arc<Shared>,
) -> std::io::Result<()> {
    let (stream, remote) = listener.accept().await?;
    let _ = stream.set_nodelay(true);
    let server = Addr::Inet(stream.local_addr()?);
    spawn_conn(
        stream,
        Addr::Inet(remote),
        server,
        builder,
        graceful,
        shared,
    );
    Ok(())
}

fn spawn_conn<S>(
    stream: S,
    remote: Addr,
    server: Addr,
    builder: &http2::Builder<TokioExecutor>,
    graceful: &GracefulShutdown,
    shared: &Arc<Shared>,
) where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let service = RapiraService::new(Arc::clone(shared), remote, server);
    let connection = builder.serve_connection(TokioIo::new(stream), service);
    let watched = graceful.watch(connection);
    tokio::spawn(async move {
        if let Err(error) = watched.await {
            tracing::debug!(target: "grpc", "connection ended with error: {error}");
        }
    });
}

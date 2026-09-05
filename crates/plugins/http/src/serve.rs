use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use anyhow::anyhow;
use extension_api::{Addr, ListenAddr, Php, PreparedListener, Result};
use hyper::server::conn::http1;
use hyper_util::rt::{TokioIo, TokioTimer};
use hyper_util::server::graceful::GracefulShutdown;
use tokio::net::TcpListener;
use tokio::sync::watch::{self, channel};

use crate::Config;
use crate::handler::{RapiraService, Shared};

pub(crate) async fn serve(
    php: Php,
    config: Config,
    prepared: PreparedListener,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    let listener = TcpListener::from_std(prepared.into_listener())?;
    match &config.listen {
        ListenAddr::Tcp(a) => tracing::info!(target: "http", "listening on http://{a}"),
    }

    let chain: Arc<[_]> = config.middleware.clone().into();
    let cfg = Arc::new(config);
    let inflight: Arc<AtomicUsize> = Arc::new(AtomicUsize::new(0));
    let shared = Arc::new(Shared {
        cfg: Arc::clone(&cfg),
        php,
        chain,
        inflight: Arc::clone(&inflight),
    });
    let graceful = GracefulShutdown::new();

    let mut builder = http1::Builder::new();
    builder
        .timer(TokioTimer::new())
        .header_read_timeout(cfg.keepalive_timeout)
        .preserve_header_case(false)
        .half_close(false)
        .keep_alive(true);

    let mut fatal: Option<anyhow::Error> = None;
    loop {
        tokio::select! {
            biased;
            _ = shutdown.wait_for(|stop| *stop) => break,
            res = accept_connection(&listener, &cfg.listen, &builder, &graceful, &shared) => match res {
                Ok(()) => {}
                Err(e) if is_fatal_accept(&e) => {
                    fatal = Some(anyhow!("listener failed: {e}"));
                    break;
                }
                Err(e) if matches!(
                    e.kind(),
                    std::io::ErrorKind::ConnectionAborted
                        | std::io::ErrorKind::ConnectionReset
                        | std::io::ErrorKind::Interrupted
                ) => {
                    tracing::debug!(target: "http", "accept skipped: {e}");
                }
                Err(e) => {
                    tracing::warn!(target: "http", "accept failed: {e}");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        }
    }

    drop(listener);
    let deadline = tokio::time::Instant::now() + cfg.drain_grace;
    if tokio::time::timeout_at(deadline, graceful.shutdown())
        .await
        .is_err()
    {
        tracing::warn!(
            target: "http",
            "graceful connection shutdown did not finish within {:?}",
            cfg.drain_grace
        );
    }
    while inflight.load(Ordering::Acquire) > 0 && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let stranded = inflight.load(Ordering::Acquire);
    if let Some(e) = fatal {
        if stranded > 0 {
            tracing::warn!(
                target: "http",
                "{stranded} request(s) still in flight when the listener failed"
            );
        }
        return Err(e);
    }
    if stranded > 0 {
        return Err(anyhow!(
            "http drain timed out after {:?} with {stranded} request(s) in flight; \
             their responses were cut short",
            cfg.drain_grace
        ));
    }
    tracing::info!(target: "http", "drained cleanly; accept loop stopped");
    Ok(())
}

// WSAENOTSOCK, WSAEINVAL, and WSAEOPNOTSUPP identify a failed listener. Windows reports WSAENOTSOCK when a listener closes during accept.
fn is_fatal_accept(e: &std::io::Error) -> bool {
    matches!(e.raw_os_error(), Some(10038 | 10022 | 10045))
}

async fn accept_connection(
    listener: &TcpListener,
    listen: &ListenAddr,
    builder: &http1::Builder,
    graceful: &GracefulShutdown,
    shared: &Arc<Shared>,
) -> std::io::Result<()> {
    let (stream, peer) = listener.accept().await?;
    let _ = stream.set_nodelay(true);
    let ListenAddr::Tcp(configured_addr) = *listen;
    let server = Addr::Inet(stream.local_addr().unwrap_or(configured_addr));
    let (closed_tx, closed_rx) = channel(false);
    let svc = RapiraService::new(Arc::clone(shared), Addr::Inet(peer), server, closed_rx);
    let io = crate::bridge::TimedIo::new(TokioIo::new(stream), shared.cfg.write_timeout);
    let connection = builder.serve_connection(io, svc);
    let watched = graceful.watch(connection);
    tokio::spawn(async move {
        if let Err(e) = watched.await {
            tracing::debug!(target: "http", "connection ended with error: {e}");
        }
        let _ = closed_tx.send(true);
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fatal_accept_errors_identify_a_failed_listener() {
        for code in [10038, 10022, 10045] {
            assert!(is_fatal_accept(&std::io::Error::from_raw_os_error(code)));
        }
        for code in [10004, 10024, 10035, 10053, 10054] {
            assert!(!is_fatal_accept(&std::io::Error::from_raw_os_error(code)));
        }
        assert!(!is_fatal_accept(&std::io::Error::other("accept failed")));
    }
}

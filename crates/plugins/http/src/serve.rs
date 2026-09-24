use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use anyhow::anyhow;
use extension_api::{Addr, ListenAddr, Php, PreparedListener, Result};
use hyper::server::conn::http1;
use hyper_util::rt::{TokioIo, TokioTimer};
use hyper_util::server::graceful::GracefulShutdown;
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::sync::watch::channel;

use crate::Config;
use crate::handler::{RapiraService, Shared};

/// Stops the accept loop. The acceptor selects on this flag.
pub(crate) struct Stop(watch::Sender<bool>);

impl Stop {
    pub(crate) fn new() -> std::io::Result<Self> {
        Ok(Self(watch::channel(false).0))
    }

    pub(crate) fn handle(&self) -> watch::Receiver<bool> {
        self.0.subscribe()
    }

    pub(crate) fn stop(&self) {
        let _ = self.0.send(true);
    }
}

/// prepare set nonblocking mode, which `from_std` requires and does not set. https://docs.rs/tokio/latest/tokio/net/struct.TcpListener.html#method.from_std
fn create_acceptor(prepared: PreparedListener) -> Result<TcpListener> {
    Ok(TcpListener::from_std(prepared.into_listener())?)
}

/// Everything the accept loop hands to a connection, and the drain that follows it.
struct Serving {
    shared: Arc<Shared>,
    graceful: GracefulShutdown,
    builder: http1::Builder,
}

impl Serving {
    fn start(php: Php, config: Config) -> Self {
        match &config.listen {
            ListenAddr::Tcp(a) => tracing::info!(target: "http", "listening on http://{a}"),
        }
        let chain: Arc<[_]> = config.middleware.clone().into();
        let shared = Arc::new(Shared {
            cfg: config,
            php,
            chain,
            inflight: Arc::new(AtomicUsize::new(0)),
        });
        let mut builder = http1::Builder::new();
        builder
            .timer(TokioTimer::new())
            .header_read_timeout(shared.cfg.keepalive_timeout);
        Self {
            shared,
            graceful: GracefulShutdown::new(),
            builder,
        }
    }

    fn spawn_tcp(&self, stream: tokio::net::TcpStream, peer: std::net::SocketAddr) {
        let _ = stream.set_nodelay(true);
        let server = stream
            .local_addr()
            .map(Addr::Inet)
            .unwrap_or_else(|_| listen_addr(&self.shared.cfg.listen));
        let (closed_tx, closed_rx) = channel(crate::bridge::ConnectionState::default());
        let svc = RapiraService::new(
            Arc::clone(&self.shared),
            Addr::Inet(peer),
            server,
            closed_rx,
        );
        let io = crate::bridge::TimedIo::new(
            TokioIo::new(stream),
            self.shared.cfg.write_timeout,
            closed_tx.clone(),
        );
        let connection = self.builder.serve_connection(io, svc);
        let watched = self.graceful.watch(connection);
        tokio::spawn(async move {
            if let Err(e) = watched.await {
                tracing::debug!(target: "http", "connection ended with error: {e}");
            }
            closed_tx.send_modify(|s| s.closed = true);
        });
    }

    /// Waits out the connections in flight. The acceptor is already gone.
    async fn drain(self, fatal: Option<anyhow::Error>) -> Result<()> {
        let deadline = tokio::time::Instant::now() + self.shared.cfg.drain_grace;
        if tokio::time::timeout_at(deadline, self.graceful.shutdown())
            .await
            .is_err()
        {
            tracing::warn!(
                target: "http",
                "graceful connection shutdown did not finish within {:?}",
                self.shared.cfg.drain_grace
            );
        }
        while self.shared.inflight.load(Ordering::Acquire) > 0
            && tokio::time::Instant::now() < deadline
        {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let stranded = self.shared.inflight.load(Ordering::Acquire);
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
                self.shared.cfg.drain_grace
            ));
        }
        tracing::info!(target: "http", "drained cleanly; accept loop stopped");
        Ok(())
    }
}

pub(crate) async fn serve(
    php: Php,
    config: Config,
    prepared: PreparedListener,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    let acceptor = create_acceptor(prepared)?;
    let serving = Serving::start(php, config);
    let mut fatal: Option<anyhow::Error> = None;
    loop {
        tokio::select! {
            biased;
            _ = shutdown.wait_for(|stop| *stop) => break,
            res = accept_connection(&acceptor, &serving) => match res {
                Ok(()) => {}
                Err(e) if is_fatal_accept(&e) => {
                    fatal = Some(anyhow!("listener failed: {e}"));
                    break;
                }
                Err(e) if is_skipped_accept(&e) => {
                    tracing::debug!(target: "http", "accept skipped: {e}");
                }
                Err(e) => {
                    tracing::warn!(target: "http", "accept failed: {e}");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        }
    }

    drop(acceptor);
    serving.drain(fatal).await
}

fn listen_addr(listen: &ListenAddr) -> Addr {
    match listen {
        ListenAddr::Tcp(a) => Addr::Inet(*a),
    }
}

// WSAENOTSOCK, WSAEINVAL, and WSAEOPNOTSUPP identify a failed listener. Windows reports WSAENOTSOCK when a listener closes during accept.
// https://learn.microsoft.com/en-us/windows/win32/winsock/windows-sockets-error-codes-2
fn is_fatal_accept(e: &std::io::Error) -> bool {
    matches!(e.raw_os_error(), Some(10038 | 10022 | 10045))
}

fn is_skipped_accept(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::Interrupted
    )
}

async fn accept_connection(acceptor: &TcpListener, serving: &Serving) -> std::io::Result<()> {
    let (stream, peer) = acceptor.accept().await?;
    serving.spawn_tcp(stream, peer);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Case {
        name: &'static str,
        error: std::io::Error,
        fatal: bool,
        skipped: bool,
    }

    /// Only an error that proves the listener is unusable ends the accept loop.
    #[test]
    fn accept_errors_end_the_loop_only_when_the_listener_is_gone() {
        let cases = [
            Case {
                name: "WSAENOTSOCK proves the listener socket is gone",
                error: std::io::Error::from_raw_os_error(10038),
                fatal: true,
                skipped: false,
            },
            Case {
                name: "WSAEINVAL proves the listener is not listening",
                error: std::io::Error::from_raw_os_error(10022),
                fatal: true,
                skipped: false,
            },
            Case {
                name: "WSAEOPNOTSUPP proves the socket type cannot accept",
                error: std::io::Error::from_raw_os_error(10045),
                fatal: true,
                skipped: false,
            },
            Case {
                name: "WSAEMFILE is a limit of this process, not of the listener",
                error: std::io::Error::from_raw_os_error(10024),
                fatal: false,
                skipped: false,
            },
            Case {
                name: "WSAECONNABORTED concerns one connection",
                error: std::io::Error::from_raw_os_error(10053),
                fatal: false,
                skipped: true,
            },
            Case {
                name: "an error without an OS code is not a listener failure",
                error: std::io::Error::other("accept failed"),
                fatal: false,
                skipped: false,
            },
        ];
        for case in cases {
            assert_eq!(is_fatal_accept(&case.error), case.fatal, "{}", case.name);
            assert_eq!(
                is_skipped_accept(&case.error),
                case.skipped,
                "{}",
                case.name
            );
        }
    }
}

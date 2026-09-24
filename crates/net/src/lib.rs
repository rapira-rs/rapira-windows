use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::anyhow;
use extension_api::{ListenAddr, PreparedListener};
use tokio::net::TcpListener;
use tokio::runtime::{Builder, Runtime};
use tokio::sync::watch;

/// Stops the accept loop. The acceptor selects on this flag.
pub struct Stop(watch::Sender<bool>);

pub type StopHandle = watch::Receiver<bool>;

impl Stop {
    pub fn new() -> std::io::Result<Self> {
        Ok(Self(watch::channel(false).0))
    }

    pub fn handle(&self) -> StopHandle {
        self.0.subscribe()
    }

    pub fn stop(&self) {
        let _ = self.0.send(true);
    }
}

/// Takes the accepted connections. [`Acceptor::run`] calls it inside its runtime.
pub trait Serve {
    fn spawn_tcp(&self, stream: tokio::net::TcpStream, peer: std::net::SocketAddr);
}

/// The listening socket of one plugin.
pub struct Acceptor {
    socket: TcpListener,
    addr: ListenAddr,
    stop: StopHandle,
}

impl Acceptor {
    /// prepare set nonblocking mode, which `from_std` requires and does not set. https://docs.rs/tokio/latest/tokio/net/struct.TcpListener.html#method.from_std
    pub fn adopt(
        prepared: PreparedListener,
        stop: StopHandle,
        rt: &Runtime,
    ) -> std::io::Result<Self> {
        let addr = prepared.addr().clone();
        // from_std registers the listener with the reactor of rt.
        let _guard = rt.enter();
        let socket = TcpListener::from_std(prepared.into_listener())?;
        Ok(Self { socket, addr, stop })
    }

    /// Runs the accept loop on rt until the stop handle fires. Returns the listener failure, if any. The listener is closed when this returns.
    pub fn run(self, rt: &Runtime, serve: &impl Serve) -> Option<anyhow::Error> {
        let Self {
            socket,
            addr,
            mut stop,
        } = self;
        let mut fatal: Option<anyhow::Error> = None;
        rt.block_on(async {
            loop {
                tokio::select! {
                    biased;
                    _ = stop.wait_for(|stop| *stop) => break,
                    res = accept_connection(&socket, serve) => match res {
                        Ok(()) => {}
                        Err(e) if is_fatal_accept(&e) => {
                            fatal = Some(anyhow!("listener failed: {e}"));
                            break;
                        }
                        Err(e) if is_skipped_accept(&e) => {
                            tracing::debug!(target: "net", "accept skipped on {addr}: {e}");
                        }
                        Err(e) => {
                            tracing::warn!(target: "net", "accept failed on {addr}: {e}");
                            tokio::time::sleep(Duration::from_millis(100)).await;
                        }
                    }
                }
            }
        });
        fatal
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

async fn accept_connection(socket: &TcpListener, serve: &impl Serve) -> std::io::Result<()> {
    let (stream, peer) = socket.accept().await?;
    let _ = stream.set_nodelay(true);
    serve.spawn_tcp(stream, peer);
    Ok(())
}

/// The server thread of one plugin: a runtime with two worker threads and the accept loop, stopped through [`Stop`].
#[derive(Default)]
pub struct ServerThread {
    stop: Option<Stop>,
    join: Option<tokio::task::JoinHandle<anyhow::Result<()>>>,
}

impl ServerThread {
    /// Spawns `rapira-{name}` and runs `body` on it with the stop handle and the runtime. Returns when the thread ends. Dropping the future leaves the thread running for [`shutdown`](Self::shutdown).
    pub async fn run(
        &mut self,
        name: &'static str,
        body: impl FnOnce(StopHandle, &Runtime) -> anyhow::Result<()> + Send + 'static,
    ) -> anyhow::Result<()> {
        let stop = Stop::new().map_err(|e| anyhow!("creating the {name} stop handle: {e}"))?;
        let handle = stop.handle();
        let thread = std::thread::Builder::new()
            .name(format!("rapira-{name}"))
            .spawn(move || {
                let rt = Builder::new_multi_thread()
                    .enable_all()
                    .worker_threads(2)
                    .thread_name(format!("rapira-{name}-io"))
                    .build()
                    .map_err(|e| anyhow!("building the {name} runtime: {e}"))?;
                body(handle, &rt)
            })?;

        self.stop = Some(stop);
        let join = self.join.insert(tokio::task::spawn_blocking(move || {
            join_thread(thread, name)
        }));
        let result = join.await;
        self.join = None;
        result.map_err(|e| anyhow!("{name} join task failed: {e}"))?
    }

    /// The thread was started and nothing joined it yet.
    pub fn is_running(&self) -> bool {
        self.join.is_some()
    }

    /// Fires the stop handle. The thread ends on its own time.
    pub fn stop(&mut self) {
        if let Some(stop) = self.stop.take() {
            stop.stop();
        }
    }

    /// Stops the thread and waits for it.
    pub async fn shutdown(&mut self) -> anyhow::Result<()> {
        self.stop();
        if let Some(join) = self.join.take() {
            join.await
                .map_err(|e| anyhow!("server join task failed: {e}"))??;
        }
        Ok(())
    }
}

/// Joins a server thread. A panic in the thread becomes an error.
fn join_thread(thread: JoinHandle<anyhow::Result<()>>, name: &str) -> anyhow::Result<()> {
    thread.join().map_err(|payload| {
        let msg = payload
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
            .unwrap_or("unknown panic");
        anyhow!("{name} server thread panicked: {msg}")
    })?
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

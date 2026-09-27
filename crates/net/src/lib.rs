use std::time::Duration;

use anyhow::anyhow;
use tokio::net::TcpListener;
use tokio::runtime::Handle;
use tokio::sync::watch;
use windows_sys::Win32::Networking::WinSock::{WSAEINVAL, WSAENOTSOCK, WSAESHUTDOWN};

pub mod listen;
pub use listen::{ListenAddr, PrepareCtx, PreparedListener};

/// Takes connections on the plugin's IO runtime.
pub trait Serve {
    fn spawn_tcp(&self, stream: tokio::net::TcpStream, peer: std::net::SocketAddr);
}

pub struct Acceptor {
    socket: TcpListener,
    addr: ListenAddr,
    stop: watch::Receiver<bool>,
}

impl Acceptor {
    pub fn adopt(
        prepared: PreparedListener,
        stop: watch::Receiver<bool>,
        rt: &Handle,
    ) -> std::io::Result<Self> {
        let addr = prepared.addr().clone();
        let _guard = rt.enter();
        Ok(Self {
            socket: TcpListener::from_std(prepared.socket)?,
            addr,
            stop,
        })
    }

    /// The socket closes before the plugin begins draining its connections.
    pub fn run(self, rt: &Handle, serve: &impl Serve) -> Option<anyhow::Error> {
        let Self {
            socket,
            addr,
            mut stop,
        } = self;
        rt.block_on(async {
            loop {
                tokio::select! {
                    biased;
                    _ = stop.wait_for(|stop| *stop) => return None,
                    res = socket.accept() => match res {
                        Ok((stream, peer)) => {
                            let _ = stream.set_nodelay(true);
                            serve.spawn_tcp(stream, peer);
                        }
                        Err(e) if is_fatal_accept(&e) => return Some(anyhow!("listener failed: {e}")),
                        Err(e) if matches!(e.kind(), std::io::ErrorKind::ConnectionAborted | std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::Interrupted) => {
                            tracing::debug!(target: "net", "accept skipped on {addr}: {e}");
                        }
                        Err(e) => {
                            tracing::warn!(target: "net", "accept failed on {addr}: {e}");
                            tokio::time::sleep(Duration::from_millis(100)).await;
                        }
                    }
                }
            }
        })
    }
}

fn is_fatal_accept(e: &std::io::Error) -> bool {
    matches!(
        e.raw_os_error(),
        Some(WSAEINVAL | WSAENOTSOCK | WSAESHUTDOWN)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::Networking::WinSock::{WSAECONNABORTED, WSAEMFILE};

    #[test]
    fn only_listener_failures_end_the_accept_loop() {
        for (code, fatal) in [
            (WSAEINVAL, true),
            (WSAENOTSOCK, true),
            (WSAESHUTDOWN, true),
            (WSAEMFILE, false),
            (WSAECONNABORTED, false),
        ] {
            assert_eq!(
                is_fatal_accept(&std::io::Error::from_raw_os_error(code)),
                fatal
            );
        }
    }
}

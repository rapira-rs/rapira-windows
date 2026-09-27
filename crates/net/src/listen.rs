use std::net::{SocketAddr, TcpListener};

use anyhow::Context;
use socket2::{Domain, Protocol, Socket, Type};

pub use rapira_config::ListenAddr;

/// A single owner carries the socket from boot to the plugin's IO runtime.
#[derive(Debug)]
pub struct PreparedListener {
    pub(crate) socket: TcpListener,
    addr: ListenAddr,
}

impl PreparedListener {
    pub fn addr(&self) -> &ListenAddr {
        &self.addr
    }
}

/// Binds every configured listener before PHP starts.
#[derive(Default)]
pub struct PrepareCtx {
    addresses: Vec<SocketAddr>,
}

impl PrepareCtx {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn bind(&mut self, addr: &ListenAddr) -> anyhow::Result<PreparedListener> {
        let ListenAddr::Tcp(addr) = addr;
        let socket = Socket::new(
            Domain::for_address(*addr),
            Type::STREAM,
            Some(Protocol::TCP),
        )
        .with_context(|| format!("socket for {addr}"))?;
        // SO_REUSEADDR on Windows lets a second process steal a live listener.
        // https://learn.microsoft.com/en-us/windows/win32/winsock/using-so-reuseaddr-and-so-exclusiveaddruse
        socket
            .bind(&(*addr).into())
            .with_context(|| format!("bind {addr}"))?;
        socket
            .listen(65535)
            .with_context(|| format!("listen {addr}"))?;
        socket.set_nonblocking(true)?;
        let resolved = socket.local_addr()?.as_socket().expect("TCP local address");
        anyhow::ensure!(
            !self.addresses.contains(&resolved),
            "duplicate listener {resolved}"
        );
        self.addresses.push(resolved);
        Ok(PreparedListener {
            socket: socket.into(),
            addr: ListenAddr::Tcp(resolved),
        })
    }
}

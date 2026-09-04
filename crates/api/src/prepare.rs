use std::net::SocketAddr;
use std::os::windows::io::{AsRawSocket, IntoRawSocket, OwnedSocket, RawSocket};

use anyhow::Context;
use socket2::{Domain, Protocol, Socket, Type};

/// The socket provider clamps this requested backlog to a supported value. https://learn.microsoft.com/en-us/windows/win32/api/winsock2/nf-winsock2-listen
pub const LISTEN_BACKLOG: i32 = 65535;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ListenAddr {
    Tcp(SocketAddr),
}

/// Owns a listener socket until the extension takes ownership.
#[derive(Debug)]
pub struct PreparedListener {
    socket: OwnedSocket,
    addr: ListenAddr,
}

impl PreparedListener {
    pub fn addr(&self) -> &ListenAddr {
        &self.addr
    }
}

impl AsRawSocket for PreparedListener {
    fn as_raw_socket(&self) -> RawSocket {
        self.socket.as_raw_socket()
    }
}

impl IntoRawSocket for PreparedListener {
    fn into_raw_socket(self) -> RawSocket {
        self.socket.into_raw_socket()
    }
}

/// Prepares listener sockets before PHP starts.
pub struct PrepareCtx {
    backlog: i32,
}

impl Default for PrepareCtx {
    fn default() -> Self {
        Self::new()
    }
}

impl PrepareCtx {
    pub fn new() -> Self {
        Self {
            backlog: LISTEN_BACKLOG,
        }
    }

    /// Sets nonblocking mode before the extension creates a Tokio listener. https://docs.rs/tokio/latest/tokio/net/struct.TcpListener.html#method.from_std
    pub fn bind_tcp(&mut self, addr: SocketAddr) -> anyhow::Result<PreparedListener> {
        let socket = Socket::new(Domain::for_address(addr), Type::STREAM, Some(Protocol::TCP))
            .with_context(|| format!("socket for {addr}"))?;
        socket
            .bind(&addr.into())
            .with_context(|| format!("bind {addr}"))?;
        socket
            .listen(self.backlog)
            .with_context(|| format!("listen {addr}"))?;
        socket.set_nonblocking(true)?;
        let resolved = socket
            .local_addr()?
            .as_socket()
            .expect("inet socket has an inet local addr");
        let addr = ListenAddr::Tcp(resolved);
        Ok(PreparedListener {
            socket: socket.into(),
            addr,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{ErrorKind, Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::os::windows::io::FromRawSocket;
    use std::sync::mpsc;
    use std::time::Duration;

    fn into_std(listener: PreparedListener) -> TcpListener {
        // SAFETY: into_raw_socket transfers ownership of the listener socket. https://doc.rust-lang.org/std/os/windows/io/trait.FromRawSocket.html#tymethod.from_raw_socket
        unsafe { TcpListener::from_raw_socket(listener.into_raw_socket()) }
    }

    #[test]
    fn tcp_bind_resolves_and_accepts() {
        let mut ctx = PrepareCtx::new();
        let l = ctx.bind_tcp("127.0.0.1:0".parse().unwrap()).unwrap();
        let ListenAddr::Tcp(resolved) = *l.addr();
        assert_ne!(resolved.port(), 0);

        let mut client = TcpStream::connect(resolved).unwrap();
        let std_l = into_std(l);
        std_l.set_nonblocking(false).unwrap();
        let (mut srv, _) = std_l.accept().unwrap();
        client.write_all(b"x").unwrap();
        let mut b = [0u8; 1];
        srv.read_exact(&mut b).unwrap();
        assert_eq!(&b, b"x");
    }

    #[test]
    fn tcp_listener_accept_is_nonblocking() {
        let mut ctx = PrepareCtx::new();
        let prepared = ctx.bind_tcp("127.0.0.1:0".parse().unwrap()).unwrap();
        let ListenAddr::Tcp(resolved) = *prepared.addr();
        let listener = into_std(prepared);
        let (tx, rx) = mpsc::channel();
        let accept = std::thread::spawn(move || {
            let _ = tx.send(listener.accept().map(|_| ()));
        });

        let result = rx.recv_timeout(Duration::from_secs(2));
        if matches!(result, Err(mpsc::RecvTimeoutError::Timeout)) {
            let _ = TcpStream::connect_timeout(&resolved, Duration::from_secs(2));
        }
        let result = result.expect("accept blocked without a pending connection");
        accept.join().unwrap();
        let error = result.unwrap_err();
        assert_eq!(error.kind(), ErrorKind::WouldBlock);
    }

    #[test]
    fn tcp_listener_drop_allows_rebind_with_live_connection() {
        let mut ctx = PrepareCtx::new();
        let prepared = ctx.bind_tcp("127.0.0.1:0".parse().unwrap()).unwrap();
        let ListenAddr::Tcp(resolved) = *prepared.addr();
        let listener = into_std(prepared);
        listener.set_nonblocking(false).unwrap();
        let mut client = TcpStream::connect(resolved).unwrap();
        let (mut server, _) = listener.accept().unwrap();
        server
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();

        drop(listener);
        let _rebound = ctx.bind_tcp(resolved).unwrap();
        client.write_all(b"open").unwrap();
        let mut body = [0; 4];
        server.read_exact(&mut body).unwrap();
        assert_eq!(&body, b"open");
    }

    #[test]
    fn duplicate_binds_rejected() {
        let mut ctx = PrepareCtx::new();
        let l = ctx.bind_tcp("127.0.0.1:0".parse().unwrap()).unwrap();
        let ListenAddr::Tcp(resolved) = *l.addr();
        assert!(ctx.bind_tcp(resolved).is_err());
        let error = PrepareCtx::new().bind_tcp(resolved).unwrap_err();
        assert_eq!(
            error.downcast_ref::<std::io::Error>().unwrap().kind(),
            ErrorKind::AddrInUse
        );
    }
}

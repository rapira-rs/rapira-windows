use std::fmt;
use std::net::{Ipv4Addr, SocketAddr};
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Listen {
    Tcp(SocketAddr),
}

impl fmt::Display for Listen {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Listen::Tcp(addr) => write!(f, "{addr}"),
        }
    }
}

#[derive(Debug)]
pub struct ListenParseError(String);

impl fmt::Display for ListenParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ListenParseError {}

impl FromStr for Listen {
    type Err = ListenParseError;

    /// The `:port` check runs before the `SocketAddr` parse. An IPv6 literal contains `:` but does not start with one.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        if !s.contains(':') {
            return Err(ListenParseError(format!(
                "`{s}` is not a listen address: use an IP address with a port or :port"
            )));
        }
        if let Some(port) = s.strip_prefix(':') {
            let port: u16 = port.parse().map_err(|_| {
                ListenParseError(format!(
                    "`{s}` has an invalid port: use an IP address with a port or :port"
                ))
            })?;
            return Ok(Listen::Tcp(SocketAddr::from((Ipv4Addr::UNSPECIFIED, port))));
        }
        s.parse::<SocketAddr>().map(Listen::Tcp).map_err(|_| {
            ListenParseError(format!(
                "`{s}` is not a listen address: use an IP address with a port or :port"
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listen_parses_all_forms() {
        assert_eq!(
            "127.0.0.1:8000".parse::<Listen>().unwrap(),
            Listen::Tcp(SocketAddr::from(([127, 0, 0, 1], 8000)))
        );
        assert_eq!(
            ":8080".parse::<Listen>().unwrap(),
            Listen::Tcp(SocketAddr::from((Ipv4Addr::UNSPECIFIED, 8080)))
        );
        assert!(matches!("[::1]:8000".parse::<Listen>(), Ok(Listen::Tcp(_))));
    }

    #[test]
    fn listen_rejects_unix_and_portless_addresses_with_supported_forms() {
        for bad in [
            "unix:/run/rapira.sock",
            "unix:",
            "localhost",
            "127.0.0.1",
            "[::1]",
            "::1",
            "2001:db8::1",
            "8080",
            "",
        ] {
            let err = bad.parse::<Listen>().unwrap_err().to_string();
            assert!(
                err.contains("use an IP address with a port or :port"),
                "{bad}: {err}"
            );
        }
    }

    #[test]
    fn listen_rejects_invalid() {
        for bad in ["8080", "", ":", "unix:", "localhost:8000"] {
            assert!(bad.parse::<Listen>().is_err(), "`{bad}` should not parse");
        }
    }
}

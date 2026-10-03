use std::net::{Ipv4Addr, SocketAddr};

/// The `:port` check runs first because an IPv6 literal never starts with ':'.
pub(crate) fn parse(s: &str) -> anyhow::Result<SocketAddr> {
    let s = s.trim();
    if s.starts_with("unix:") {
        anyhow::bail!("Unix listeners are not supported on Windows: use host:port or :port");
    }
    if !s.contains(':') {
        anyhow::bail!("`{s}` is not a listen address: use host:port or :port");
    }
    if let Some(port) = s.strip_prefix(':') {
        let port: u16 = port
            .parse()
            .map_err(|_| anyhow::anyhow!("`{s}` has an invalid port"))?;
        return Ok(SocketAddr::from((Ipv4Addr::UNSPECIFIED, port)));
    }
    s.parse().map_err(|_| {
        anyhow::anyhow!("`{s}` is not host:port (expected an IP literal, e.g. 127.0.0.1:8000)")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listen_parses_all_forms() {
        assert_eq!(
            parse("127.0.0.1:8000").unwrap(),
            SocketAddr::from(([127, 0, 0, 1], 8000))
        );
        assert_eq!(
            parse(":8080").unwrap(),
            SocketAddr::from((Ipv4Addr::UNSPECIFIED, 8080))
        );
        assert!(parse("[::1]:8000").is_ok());
    }

    #[test]
    fn listen_rejects_invalid() {
        for bad in [
            "8080",
            "",
            ":",
            "unix:",
            "unix:/run/r.sock",
            "localhost:8000",
        ] {
            assert!(parse(bad).is_err(), "`{bad}` should not parse");
        }
    }
}

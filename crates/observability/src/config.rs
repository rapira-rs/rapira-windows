use std::net::SocketAddr;
use std::time::Duration;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Section {
    pub listen: String,
    pub keepalive_timeout_secs: Option<u64>,
    pub metrics: Option<Endpoints>,
    pub probes: Option<Endpoints>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Endpoints {}

#[derive(Debug)]
pub struct Settings {
    pub listen: SocketAddr,
    pub keepalive_timeout: Duration,
    pub metrics: bool,
    pub probes: bool,
}

pub fn resolve(section: Section) -> anyhow::Result<Settings> {
    let listen = rapira_config::parse_listen(
        "observability",
        Some(&section.listen),
        SocketAddr::from(([0, 0, 0, 0], 0)),
    )?;
    let keepalive_timeout = rapira_config::nonzero_timeout(
        "observability",
        "keepalive_timeout_secs",
        section.keepalive_timeout_secs.unwrap_or(60),
    )?;
    if section.metrics.is_none() && section.probes.is_none() {
        anyhow::bail!("[observability] needs [observability.metrics] or [observability.probes]");
    }
    Ok(Settings {
        listen,
        keepalive_timeout,
        metrics: section.metrics.is_some(),
        probes: section.probes.is_some(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_address_timeout_and_endpoints() {
        struct Case {
            name: &'static str,
            toml: &'static str,
            expected: Result<(SocketAddr, u64, bool, bool), &'static str>,
        }
        let cases = [
            Case {
                name: "metrics and probes on all interfaces",
                toml: "listen = ':9180'\nkeepalive_timeout_secs = 7\n[metrics]\n[probes]",
                expected: Ok((SocketAddr::from(([0, 0, 0, 0], 9180)), 7, true, true)),
            },
            Case {
                name: "probes with default timeout",
                toml: "listen = '127.0.0.1:9180'\n[probes]",
                expected: Ok((SocketAddr::from(([127, 0, 0, 1], 9180)), 60, false, true)),
            },
            Case {
                name: "missing listen",
                toml: "[metrics]",
                expected: Err("missing field `listen`"),
            },
            Case {
                name: "bad address",
                toml: "listen = 'localhost'\n[metrics]",
                expected: Err(
                    "invalid observability.listen `localhost`: `localhost` is not a listen address: use host:port or :port",
                ),
            },
            Case {
                name: "zero timeout",
                toml: "listen = ':9180'\nkeepalive_timeout_secs = 0\n[metrics]",
                expected: Err("observability.keepalive_timeout_secs must be at least 1"),
            },
            Case {
                name: "timeout above cap",
                toml: "listen = ':9180'\nkeepalive_timeout_secs = 100000\n[metrics]",
                expected: Err(
                    "observability.keepalive_timeout_secs 100000 is too large (max 86400)",
                ),
            },
            Case {
                name: "no endpoint table",
                toml: "listen = ':9180'",
                expected: Err(
                    "[observability] needs [observability.metrics] or [observability.probes]",
                ),
            },
            Case {
                name: "unknown metrics key",
                toml: "listen = ':9180'\n[metrics]\npath = '/stats'",
                expected: Err("unknown field `path`"),
            },
        ];
        for case in cases {
            let got = toml::from_str::<Section>(case.toml)
                .map_err(anyhow::Error::from)
                .and_then(resolve);
            match (got, case.expected) {
                (Ok(s), Ok(want)) => assert_eq!(
                    (s.listen, s.keepalive_timeout.as_secs(), s.metrics, s.probes),
                    want,
                    "{}",
                    case.name
                ),
                (Err(e), Err(want)) => {
                    assert!(format!("{e:#}").contains(want), "{}: {e:#}", case.name)
                }
                (got, _) => panic!("{}: unexpected {got:?}", case.name),
            }
        }
    }
}

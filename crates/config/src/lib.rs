use anyhow::{Context, bail};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

mod listen;
mod log;
mod pool;
mod supervisor;

pub use log::{LogFormat, LogLevel, LogSettings, resolve_log};
pub use pool::{Mode, PoolSection, PoolSettings, check_entrypoint, resolve_pool};
pub use supervisor::{SupervisorSection, SupervisorSettings, resolve_supervisor};

/// Resolve `value` against the config directory. An empty string counts as absent.
pub fn opt_path(dir: &Path, value: Option<&str>) -> std::io::Result<Option<PathBuf>> {
    value
        .filter(|v| !v.is_empty())
        .map(|v| std::path::absolute(dir.join(v)))
        .transpose()
}

/// `{table}.listen`, or `default` when the key is absent.
pub fn parse_listen(
    table: &str,
    raw: Option<&str>,
    default: SocketAddr,
) -> anyhow::Result<SocketAddr> {
    match raw {
        Some(s) => listen::parse(s).with_context(|| format!("invalid {table}.listen `{s}`")),
        None => Ok(default),
    }
}

/// A `_secs` key that must be at least 1 and at most `MAX_TIMEOUT_SECS`.
pub fn nonzero_timeout(table: &str, key: &str, secs: u64) -> anyhow::Result<Duration> {
    if secs == 0 {
        bail!("{table}.{key} must be at least 1");
    }
    capped_timeout(table, key, secs)
}

/// Caps every `*_secs` key so the host's deadline arithmetic cannot overflow.
const MAX_TIMEOUT_SECS: u64 = 86_400;

pub fn capped_timeout(table: &str, key: &str, secs: u64) -> anyhow::Result<Duration> {
    if secs > MAX_TIMEOUT_SECS {
        bail!("{table}.{key} {secs} is too large (max {MAX_TIMEOUT_SECS})");
    }
    Ok(Duration::from_secs(secs))
}

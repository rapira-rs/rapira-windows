use anyhow::bail;
use serde::Deserialize;
use std::path::{Path, PathBuf};

use crate::config_relative;

#[derive(Debug)]
pub struct PoolSettings {
    pub entrypoint: PathBuf,
    /// PHP interpreter threads in one process.
    pub processes: usize,
    pub mode: RunMode,
    /// Number of requests that a worker serves before recycling, including jitter. A value of 0 has no limit.
    pub max_requests: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunMode {
    Classic,
    Worker,
    #[default]
    Dispatcher,
}

impl RunMode {
    pub fn as_str(self) -> &'static str {
        match self {
            RunMode::Classic => "classic",
            RunMode::Worker => "worker",
            RunMode::Dispatcher => "dispatcher",
        }
    }
}

/// Embedded by name because serde does not support `#[serde(flatten)]` together with `deny_unknown_fields`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PoolSection {
    entrypoint: Option<String>,
    processes: Option<usize>,
    mode: Option<RunMode>,
    max_requests: Option<u64>,
}

fn default_processes() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
}

/// `table` is the qualified table of the calling plugin, such as `http.pool`. Every message contains it.
pub(crate) fn resolve_pool(
    section: PoolSection,
    table: &str,
    config_dir: Option<&Path>,
) -> anyhow::Result<PoolSettings> {
    let processes = section.processes.unwrap_or_else(default_processes);
    if processes == 0 {
        bail!("{table}.processes must be at least 1");
    }

    let mode = section.mode.unwrap_or_default();

    let Some(ep) = section.entrypoint.as_deref().filter(|s| !s.is_empty()) else {
        bail!("{table}.entrypoint is required");
    };
    let entrypoint = config_relative(config_dir, ep)?;

    Ok(PoolSettings {
        entrypoint,
        processes,
        mode,
        max_requests: section.max_requests.unwrap_or(0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every message contains the table of the caller, so a second pool reports its own keys.
    #[test]
    fn resolve_pool_prefixes_errors_with_the_table() {
        let err = resolve_pool(PoolSection::default(), "grpc.pool", None)
            .unwrap_err()
            .to_string();
        assert_eq!(err, "grpc.pool.entrypoint is required");
    }
}

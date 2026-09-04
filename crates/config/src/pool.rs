use anyhow::bail;
use serde::Deserialize;
use std::path::{Path, PathBuf};

use crate::{Overrides, config_relative};

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

impl std::str::FromStr for RunMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "classic" => Ok(RunMode::Classic),
            "worker" => Ok(RunMode::Worker),
            "dispatcher" => Ok(RunMode::Dispatcher),
            other => Err(format!(
                "unknown mode `{other}` (expected classic, worker, or dispatcher)"
            )),
        }
    }
}

/// This section uses a named field because serde does not support `#[serde(flatten)]` with `deny_unknown_fields`.
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

/// `table` is the key path in error messages. `cli` applies only to the root pool. Pass `&Overrides::default()` for another pool.
pub(crate) fn resolve_pool(
    section: PoolSection,
    cli: &Overrides,
    config_dir: Option<&Path>,
    table: &str,
) -> anyhow::Result<PoolSettings> {
    let processes = cli
        .processes
        .or(section.processes)
        .unwrap_or_else(default_processes);
    if processes == 0 {
        bail!("{table}.processes must be at least 1");
    }

    let mode = cli.mode.or(section.mode).unwrap_or_default();

    let entrypoint = if let Some(script) = &cli.entrypoint {
        std::path::absolute(script)?
    } else if let Some(ep) = section.entrypoint.as_deref().filter(|s| !s.is_empty()) {
        config_relative(config_dir, ep)?
    } else {
        bail!("no entrypoint: pass a SCRIPT argument or set {table}.entrypoint in the config file");
    };

    Ok(PoolSettings {
        entrypoint,
        processes,
        mode,
        max_requests: section.max_requests.unwrap_or(0),
    })
}

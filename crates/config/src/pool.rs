use anyhow::bail;
use serde::Deserialize;
use std::fmt;
use std::fs::File;
use std::path::{Path, PathBuf};

use crate::ConfigCtx;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolSettings {
    pub entrypoint: PathBuf,
    /// Fixed interpreter thread count for this plugin.
    pub processes: usize,
    pub mode: Mode,
    /// Requests per interpreter generation, with jitter; zero is unlimited.
    pub max_requests: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Classic,
    Worker,
    Dispatcher,
}

impl fmt::Display for Mode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Classic => "classic",
            Self::Worker => "worker",
            Self::Dispatcher => "dispatcher",
        })
    }
}

/// Embedded by name: serde does not support flatten with deny_unknown_fields.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PoolSection {
    entrypoint: Option<String>,
    processes: Option<usize>,
    mode: Option<Mode>,
    max_requests: Option<u64>,
}

pub fn resolve_pool(
    section: PoolSection,
    table: &str,
    ctx: &ConfigCtx,
) -> anyhow::Result<PoolSettings> {
    let processes = section
        .processes
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, |n| n.get()));
    if !(1..=4096).contains(&processes) {
        bail!("{table}.processes must be between 1 and 4096");
    }
    let Some(ep) = section.entrypoint.as_deref().filter(|s| !s.is_empty()) else {
        bail!("{table}.entrypoint is required");
    };
    Ok(PoolSettings {
        entrypoint: ctx.resolve_path(ep)?,
        processes,
        mode: section.mode.unwrap_or(Mode::Dispatcher),
        max_requests: section.max_requests.unwrap_or(0),
    })
}

pub fn check_entrypoint(table: &str, entrypoint: &Path) -> anyhow::Result<()> {
    let meta = File::open(entrypoint)
        .and_then(|f| f.metadata())
        .map_err(|e| {
            anyhow::anyhow!(
                "{table}.entrypoint {} is not readable: {e}",
                entrypoint.display()
            )
        })?;
    anyhow::ensure!(
        meta.is_file(),
        "{table}.entrypoint {} is not a regular file",
        entrypoint.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pool(text: &str) -> anyhow::Result<PoolSettings> {
        resolve_pool(
            toml::from_str(text)?,
            "http.pool",
            &ConfigCtx {
                dir: PathBuf::from("C:/app"),
            },
        )
    }

    #[test]
    fn pool_paths_modes_and_quotas_resolve() {
        let got = pool(
            "entrypoint = 'public/index.php'\nprocesses = 4\nmode = 'worker'\nmax_requests = 200",
        )
        .unwrap();
        assert_eq!(got.entrypoint, Path::new("C:/app/public/index.php"));
        assert_eq!(
            (got.processes, got.mode, got.max_requests),
            (4, Mode::Worker, 200)
        );
        assert_eq!(
            pool("entrypoint = 'index.php'").unwrap().mode,
            Mode::Dispatcher
        );
    }

    #[test]
    fn invalid_pool_settings_name_the_key() {
        for (text, key) in [
            ("", "entrypoint"),
            ("entrypoint = ''", "entrypoint"),
            ("entrypoint = 'a.php'\nprocesses = 0", "processes"),
            ("entrypoint = 'a.php'\nprocesses = 4097", "processes"),
            ("scaling = 'dynamic'", "scaling"),
            (
                "request_terminate_timeout_secs = 1",
                "request_terminate_timeout_secs",
            ),
        ] {
            assert!(pool(text).unwrap_err().to_string().contains(key));
        }
    }
}

use anyhow::bail;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::{capped_timeout, config_relative};

#[derive(Debug)]
pub struct SupervisorSettings {
    pub process_control_timeout: Duration,
    pub pidfile: Option<PathBuf>,
}

impl SupervisorSettings {
    /// The margin lets HTTP draining complete before `process_control_timeout` expires.
    pub fn drain_grace(&self) -> Duration {
        const MARGIN: Duration = Duration::from_secs(5);
        let margin = MARGIN.min(self.process_control_timeout / 2);
        self.process_control_timeout - margin
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SupervisorSection {
    pidfile: Option<String>,
    process_control_timeout_secs: Option<u64>,
}

pub(crate) fn resolve_supervisor(
    section: SupervisorSection,
    config_dir: Option<&Path>,
) -> anyhow::Result<SupervisorSettings> {
    let pidfile = section
        .pidfile
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(|p| config_relative(config_dir, p))
        .transpose()?;

    let control_secs = section.process_control_timeout_secs.unwrap_or(30);
    if control_secs == 0 {
        bail!("supervisor.process_control_timeout_secs must be at least 1");
    }

    Ok(SupervisorSettings {
        process_control_timeout: capped_timeout(
            "supervisor",
            "process_control_timeout_secs",
            control_secs,
        )?,
        pidfile,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drain_grace_margin_keeps_http_inside_runtime_timeout_budget() {
        let grace = |secs| {
            SupervisorSettings {
                process_control_timeout: Duration::from_secs(secs),
                pidfile: None,
            }
            .drain_grace()
        };
        assert_eq!(grace(30), Duration::from_secs(25));
        assert_eq!(grace(60), Duration::from_secs(55));
        assert_eq!(grace(5), Duration::from_millis(2500));
        assert_eq!(grace(1), Duration::from_millis(500));
        for secs in 1..=120 {
            assert!(
                grace(secs) < Duration::from_secs(secs),
                "http drain must finish inside the runtime timeout budget of {secs}s"
            );
        }
    }
}

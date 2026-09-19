use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use anyhow::Context;
use php_sys::{Mode, PoolHooks, Rapira};
use rapira_runtime::{ExtensionRuntime, ShutdownWatcher, Stopper};

pub struct WorkerOutcome {
    pub code: u8,
    pub joined: bool,
    _shutdown: ShutdownWatcher,
}

fn exit_code(boot_failed: bool, outcomes: Option<&[Result<(), String>]>) -> u8 {
    if boot_failed {
        70
    } else if outcomes.is_some_and(|outcomes| outcomes.iter().all(Result::is_ok)) {
        0
    } else {
        1
    }
}

fn remove_spool_dir(dir: Option<&Path>) {
    if let Some(dir) = dir
        && let Err(e) = std::fs::remove_dir_all(dir)
    {
        tracing::warn!(target: "rapira", "removing spool dir {}: {e}", dir.display());
    }
}

pub struct PoolArgs {
    pub host: ExtensionRuntime,
    pub mode: Mode,
    pub script: PathBuf,
    pub processes: usize,
    pub max_requests: u64,
    pub uploads: Option<rapira_runtime::multipart::Limits>,
}

/// Starts the protocol thread groups under one PHP module and one stop signal.
pub fn worker_body(mut pools: Vec<PoolArgs>, grace: Duration) -> anyhow::Result<WorkerOutcome> {
    let shutdown = ShutdownWatcher::install().context("installing console control handler")?;
    let mut options = rapira_runtime::RuntimeOptions {
        grace,
        ..Default::default()
    };
    let mut spool_dir = None;
    for pool in &mut pools {
        if let Some(mut uploads) = pool.uploads.take() {
            if matches!(pool.mode, Mode::Dispatcher(_)) {
                uploads.dir = uploads
                    .dir
                    .join(format!("rapira-spool-{}", std::process::id()));
                std::fs::create_dir(&uploads.dir)
                    .with_context(|| format!("creating spool dir {}", uploads.dir.display()))?;
                spool_dir = Some(uploads.dir.clone());
            }
            options.uploads = Arc::new(uploads);
        }
    }

    let boot_failed = Arc::new(AtomicBool::new(false));
    let stopper: Arc<OnceLock<Stopper>> = Arc::new(OnceLock::new());
    let configs = pools
        .iter()
        .map(|pool| php_sys::PoolConfig {
            mode: pool.mode.clone(),
            processes: pool.processes,
            hooks: PoolHooks {
                max_requests: pool.max_requests,
                on_boot_failure: Arc::new({
                    let boot_failed = boot_failed.clone();
                    let stopper = stopper.clone();
                    move || {
                        boot_failed.store(true, SeqCst);
                        if let Some(stopper) = stopper.get() {
                            stopper.stop();
                        }
                    }
                }),
            },
        })
        .collect();

    let rapira = match Rapira::start_pools(configs) {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(target: "rapira", "PHP pool boot failed: {e:#}");
            remove_spool_dir(spool_dir.as_deref());
            return Ok(WorkerOutcome {
                code: 70,
                joined: true,
                _shutdown: shutdown,
            });
        }
    };
    let outcomes = catch_unwind(AssertUnwindSafe(|| {
        let hosts = pools
            .into_iter()
            .enumerate()
            .map(|(index, pool)| (pool.host, rapira.pool_handle(index), pool.script))
            .collect();
        let running = ExtensionRuntime::run_pools(hosts, options);
        let _ = stopper.set(running.stopper());
        // PHP can fail before the stopper is registered.
        if boot_failed.load(SeqCst) {
            running.stopper().stop();
        }
        running.serve(&shutdown)
    }));

    match &outcomes {
        Ok(outcomes) => {
            for error in outcomes.iter().filter_map(|outcome| outcome.as_ref().err()) {
                tracing::error!(target: "rapira", "extension failed: {error}");
            }
        }
        Err(_) => tracing::error!(target: "rapira", "extension runtime panicked"),
    }
    let code = exit_code(
        boot_failed.load(SeqCst),
        outcomes.as_ref().ok().map(Vec::as_slice),
    );
    let joined = rapira.shutdown();
    if joined {
        remove_spool_dir(spool_dir.as_deref());
    }
    Ok(WorkerOutcome {
        code,
        joined,
        _shutdown: shutdown,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boot_failure_takes_precedence_over_extension_errors_and_panics() {
        assert_eq!(exit_code(true, Some(&[Ok(())])), 70);
        assert_eq!(
            exit_code(true, Some(&[Err("shutdown timed out".into())])),
            70
        );
        assert_eq!(exit_code(true, None), 70);
    }

    #[test]
    fn extension_errors_and_panics_exit_one() {
        assert_eq!(exit_code(false, Some(&[Ok(())])), 0);
        assert_eq!(
            exit_code(false, Some(&[Err("shutdown timed out".into())])),
            1
        );
        assert_eq!(exit_code(false, None), 1);
    }
}

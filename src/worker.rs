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

// The registration path rechecks the flag if PHP fails before the stopper exists.
fn request_boot_failure(failed: &AtomicBool, stopper: &OnceLock<Stopper>) {
    failed.store(true, SeqCst);
    if let Some(s) = stopper.get() {
        s.stop();
    }
}

fn stop_if_boot_failed(failed: &AtomicBool, stop: impl FnOnce()) {
    if failed.load(SeqCst) {
        stop();
    }
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

/// Runs the thread pool on its boot thread and reports whether PHP teardown completed.
pub fn worker_body(
    host: ExtensionRuntime,
    mode: Mode,
    script: PathBuf,
    processes: usize,
    max_requests: u64,
    mut uploads: rapira_runtime::multipart::Limits,
    grace: Duration,
) -> anyhow::Result<WorkerOutcome> {
    let shutdown = ShutdownWatcher::install().context("installing console control handler")?;
    let spool_dir = if matches!(mode, Mode::Dispatcher(_)) {
        uploads.dir = uploads
            .dir
            .join(format!("rapira-spool-{}", std::process::id()));
        std::fs::create_dir(&uploads.dir)
            .with_context(|| format!("creating spool dir {}", uploads.dir.display()))?;
        Some(uploads.dir.clone())
    } else {
        None
    };

    let boot_failed = Arc::new(AtomicBool::new(false));
    let stopper: Arc<OnceLock<Stopper>> = Arc::new(OnceLock::new());
    let hooks = PoolHooks {
        max_requests,
        on_boot_failure: Arc::new({
            let boot_failed = boot_failed.clone();
            let stopper = stopper.clone();
            move || request_boot_failure(&boot_failed, &stopper)
        }),
    };

    let rapira = match Rapira::start_pool(mode, processes, hooks) {
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
    let handle = rapira.handle();

    let outcomes = catch_unwind(AssertUnwindSafe(|| {
        let running = host.run_with_options(
            handle,
            script,
            rapira_runtime::RuntimeOptions {
                uploads: Arc::new(uploads),
                grace,
            },
        );
        let _ = stopper.set(running.stopper());
        stop_if_boot_failed(&boot_failed, || stopper.get().expect("just set").stop());
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

    #[test]
    fn early_boot_failure_stops_after_registration() {
        let failed = AtomicBool::new(false);
        let stopper = OnceLock::new();
        request_boot_failure(&failed, &stopper);

        let stopped = AtomicBool::new(false);
        stop_if_boot_failed(&failed, || stopped.store(true, SeqCst));
        assert!(stopped.load(SeqCst));
    }

    #[test]
    fn successful_boot_does_not_stop_after_registration() {
        stop_if_boot_failed(&AtomicBool::new(false), || panic!("unexpected stop"));
    }
}

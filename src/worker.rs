use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use anyhow::Context;
use php_sys::{Mode, PoolHooks, PoolSpec, Rapira};
use rapira_runtime::{ExtensionRuntime, Running, ShutdownWatcher, Stopper};

pub struct WorkerOutcome {
    pub code: u8,
    pub joined: bool,
    _shutdown: ShutdownWatcher,
}

/// The pool settings of one plugin table.
pub struct PoolArgs {
    pub mode: Mode,
    pub entrypoint: PathBuf,
    pub threads: usize,
    pub max_requests: u64,
    /// Multipart limits with the spool directory. Only an HTTP dispatcher pool parses uploads.
    pub uploads: Option<rapira_runtime::multipart::Limits>,
}

/// One plugin table: its extension host with the bound listener, and its interpreter thread pool.
pub struct PoolRun {
    /// `http` or `grpc`: the pool name in thread names and log lines.
    pub name: &'static str,
    pub host: ExtensionRuntime,
    pub args: PoolArgs,
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

/// Boots PHP once, starts every pool, serves every extension host on its own thread, and reports whether PHP teardown completed.
pub fn worker_body(mut pools: Vec<PoolRun>, grace: Duration) -> anyhow::Result<WorkerOutcome> {
    let shutdown = ShutdownWatcher::install().context("installing console control handler")?;
    // Every request starts in the process working directory because ZTS PHP resets the thread cwd at request startup. The first pool is the HTTP pool when the configuration has one. The boot check accepted a regular file, so the parent exists.
    let first = pools
        .first()
        .expect("the configuration names at least one plugin table");
    let entrypoint_dir: &Path = first
        .args
        .entrypoint
        .parent()
        .context("the entrypoint has no parent directory")?;
    std::env::set_current_dir(entrypoint_dir).with_context(|| {
        format!(
            "entering the entrypoint directory {}",
            entrypoint_dir.display()
        )
    })?;
    // Classic and worker requests read the script paths from one process-wide value, so only the HTTP pool sets them.
    if let Some(http) = pools.iter().find(|pool| pool.name == "http") {
        php_sys::set_script(&http.args.entrypoint);
    }
    let mut spool_dir: Option<PathBuf> = None;
    if let Some(uploads) = pools.iter_mut().find_map(|pool| pool.args.uploads.as_mut()) {
        uploads.dir = uploads
            .dir
            .join(format!("rapira-spool-{}", std::process::id()));
        std::fs::create_dir(&uploads.dir)
            .with_context(|| format!("creating spool dir {}", uploads.dir.display()))?;
        spool_dir = Some(uploads.dir.clone());
    }

    // A boot failure in any pool stops every extension host, so the process exits with code 70.
    let boot_failed = Arc::new(AtomicBool::new(false));
    let stoppers: Arc<OnceLock<Vec<Stopper>>> = Arc::new(OnceLock::new());
    let on_boot_failure: Arc<dyn Fn() + Send + Sync> = Arc::new({
        let boot_failed = boot_failed.clone();
        let stoppers = stoppers.clone();
        move || {
            boot_failed.store(true, SeqCst);
            if let Some(stoppers) = stoppers.get() {
                for stopper in stoppers {
                    stopper.stop();
                }
            }
        }
    });

    let total_threads: usize = pools.iter().map(|pool| pool.args.threads).sum();
    let mut rapira = match Rapira::boot(total_threads) {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(target: "rapira", "PHP boot failed: {e:#}");
            remove_spool_dir(spool_dir.as_deref());
            return Ok(WorkerOutcome {
                code: 70,
                joined: true,
                _shutdown: shutdown,
            });
        }
    };
    let mut hosts = Vec::with_capacity(pools.len());
    for PoolRun { name, host, args } in pools {
        let spec = PoolSpec {
            name,
            mode: args.mode,
            threads: args.threads,
            hooks: PoolHooks {
                max_requests: args.max_requests,
                on_boot_failure: on_boot_failure.clone(),
            },
        };
        let handle = match rapira.add_pool(spec) {
            Ok(handle) => handle,
            Err(e) => {
                tracing::error!(target: "rapira", "PHP pool {name} boot failed: {e:#}");
                let joined = rapira.shutdown();
                if joined {
                    remove_spool_dir(spool_dir.as_deref());
                }
                return Ok(WorkerOutcome {
                    code: 70,
                    joined,
                    _shutdown: shutdown,
                });
            }
        };
        hosts.push((host, handle, args.uploads));
    }

    let outcomes = catch_unwind(AssertUnwindSafe(|| {
        let runnings: Vec<Running> = hosts
            .into_iter()
            .map(|(host, handle, uploads)| {
                host.run_with_options(
                    handle,
                    rapira_runtime::RuntimeOptions {
                        uploads: Arc::new(uploads.unwrap_or_default()),
                        grace,
                    },
                )
            })
            .collect();
        let _ = stoppers.set(runnings.iter().map(Running::stopper).collect());
        // PHP can fail before the stoppers are registered.
        if boot_failed.load(SeqCst) {
            for running in &runnings {
                running.stopper().stop();
            }
        }
        // `serve` blocks on the runtime of its host, so each host serves on its own thread.
        let shutdown = &shutdown;
        std::thread::scope(|scope| {
            let served: Vec<_> = runnings
                .into_iter()
                .map(|running| scope.spawn(move || running.serve(shutdown)))
                .collect();
            served
                .into_iter()
                .flat_map(|thread| {
                    thread
                        .join()
                        .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
                })
                .collect::<Vec<Result<(), String>>>()
        })
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

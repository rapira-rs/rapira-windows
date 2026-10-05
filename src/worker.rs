use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use rapira_config::{PoolSettings, SupervisorSettings};
use rapira_sapi::Rapira;
use rapira_sapi::plugin::{Plugin, Stopper, run_plugin};

mod console {
    use super::*;
    use windows_sys::Win32::System::Console::{
        CTRL_BREAK_EVENT, CTRL_C_EVENT, SetConsoleCtrlHandler,
    };
    use windows_sys::core::BOOL;

    pub(super) static ASKED: AtomicBool = AtomicBool::new(false);
    unsafe extern "system" fn handler(event: u32) -> BOOL {
        match event {
            CTRL_C_EVENT | CTRL_BREAK_EVENT => {
                if ASKED.swap(true, Ordering::SeqCst) {
                    super::force_exit(130);
                }
                1
            }
            _ => 0,
        }
    }

    pub(super) struct Watcher;
    impl Watcher {
        pub(super) fn install() -> std::io::Result<Self> {
            // Windows runs handler on a separate thread. It stays installed through PHP teardown.
            // https://learn.microsoft.com/en-us/windows/console/setconsolectrlhandler
            if unsafe { SetConsoleCtrlHandler(Some(handler), 1) } == 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(Self)
        }
    }
    impl Drop for Watcher {
        fn drop(&mut self) {
            unsafe {
                SetConsoleCtrlHandler(Some(handler), 0);
            }
        }
    }
}

fn force_exit(code: u32) -> ! {
    // ExitProcess invokes DLL detach under locks that an active PHP thread may own.
    // https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-terminateprocess
    unsafe {
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
        TerminateProcess(GetCurrentProcess(), code);
    }
    std::process::abort();
}

pub fn serve(
    plugins: Vec<(Box<dyn Plugin>, PoolSettings)>,
    threads: usize,
    supervisor: &SupervisorSettings,
    observability: Option<rapira_observability::Server>,
) -> anyhow::Result<u8> {
    let _console = console::Watcher::install()?;
    let module = rapira_sapi::boot_master(
        &[
            rapira_http::rapira_http_register_classes,
            rapira_grpc::rapira_grpc_register_classes,
        ],
        threads,
    )?;
    let board = rapira_scoreboard::create(threads)?;
    let boot_failed = Arc::new(AtomicBool::new(false));
    let mut pools = Vec::new();
    let mut running = Vec::new();
    let mut stoppers = Vec::new();
    let mut offset = 0;
    let boards: Vec<_> = plugins
        .iter()
        .map(|(plugin, settings)| {
            let slots = Arc::new(rapira_scoreboard::Pool::new(
                plugin.name(),
                &board[offset..offset + settings.processes],
            ));
            offset += settings.processes;
            slots
        })
        .collect();
    let obs_stop = tokio::sync::watch::Sender::new(false);
    let mut observer = None;
    let startup: anyhow::Result<()> = (|| {
        if let Some(server) = observability {
            let pools = boards.clone();
            let stop = obs_stop.subscribe();
            let grace = supervisor.drain_grace();
            observer = Some(
                std::thread::Builder::new()
                    .name("rapira-observability".into())
                    .spawn(move || server.serve(pools, stop, grace))?,
            );
        }
        for ((plugin, settings), slots) in plugins.into_iter().zip(&boards) {
            let pool = Rapira::start_pool(
                &module,
                settings.mode,
                settings.entrypoint,
                slots.clone(),
                settings.max_requests,
                boot_failed.clone(),
                plugin.dispatcher(),
            )?;
            let stopper = Stopper::default();
            let sink = pool.sink();
            pools.push(pool);
            running.push(run_plugin(
                plugin,
                sink,
                stopper.clone(),
                supervisor.drain_grace(),
            )?);
            stoppers.push(stopper);
        }
        Ok(())
    })();
    while startup.is_ok() {
        if console::ASKED.load(Ordering::Acquire)
            || boot_failed.load(Ordering::Acquire)
            || running.iter().any(|plugin| plugin.is_finished())
            || observer.as_ref().is_some_and(|thread| thread.is_finished())
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    tracing::info!(target: "rapira", "shutdown requested; draining plugins");
    let mut failed = observer.as_ref().is_some_and(|thread| thread.is_finished());
    if failed {
        tracing::error!(target: "observability", "server stopped before shutdown");
    }
    let deadline = Instant::now() + supervisor.process_control_timeout;
    // The same deadline covers plugin drain, interpreter teardown, and module shutdown.
    let (_shutdown_guard, shutdown_done) = std::sync::mpsc::channel::<()>();
    let failed_boot = boot_failed.clone();
    if let Err(error) = std::thread::Builder::new()
        .name("rapira-shutdown".into())
        .spawn(move || {
            if matches!(
                shutdown_done.recv_timeout(deadline.saturating_duration_since(Instant::now())),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout)
            ) {
                force_exit(if failed_boot.load(Ordering::Acquire) {
                    70
                } else {
                    1
                });
            }
        })
    {
        tracing::error!(target: "rapira", "cannot start the shutdown timer: {error}");
        force_exit(1);
    }
    for stopper in &stoppers {
        stopper.stop();
    }
    obs_stop.send_replace(true);
    for plugin in running {
        if let Err(error) = plugin.join() {
            tracing::error!(target: "rapira", "{error:#}");
            failed = true;
        }
    }
    if let Some(thread) = observer {
        let result = thread
            .join()
            .map_err(|_| anyhow::anyhow!("observability thread panicked"))
            .and_then(|result| result);
        if let Err(error) = result {
            tracing::error!(target: "observability", "{error:#}");
            failed = true;
        }
    }
    for pool in &mut pools {
        pool.stop();
    }
    for pool in &mut pools {
        if !pool.join(deadline) {
            tracing::error!(target: "rapira", "PHP pool did not stop within the shutdown budget");
            force_exit(if boot_failed.load(Ordering::Acquire) {
                70
            } else {
                1
            });
        }
    }
    for board in boards {
        for (id, slot) in board.slots.iter().enumerate() {
            tracing::info!(target: "rapira", pool = board.name, "slot {id} pid {} state {} handled {} errors {} recycles {}", slot.pid.load(Ordering::Relaxed), slot.state.load(Ordering::Relaxed), slot.handled.load(Ordering::Relaxed), slot.errors.load(Ordering::Relaxed), slot.recycles.load(Ordering::Relaxed));
        }
    }
    drop(pools);
    drop(module);
    startup?;
    Ok(if boot_failed.load(Ordering::Acquire) {
        70
    } else if failed {
        1
    } else {
        0
    })
}

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use rapira_config::{PoolSettings, SupervisorSettings};
use rapira_sapi::plugin::{Plugin, Stopper, run_plugin};
use rapira_sapi::{PoolHooks, Rapira};

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
) -> anyhow::Result<u8> {
    let _console = console::Watcher::install()?;
    let module =
        rapira_sapi::boot_master(&[rapira_http::PHP_PART, rapira_grpc::PHP_PART], threads)?;
    let board = rapira_scoreboard::Scoreboard::create(threads)?;
    let boot_failed = Arc::new(AtomicBool::new(false));
    let mut pools = Vec::new();
    let mut running = Vec::new();
    let mut stoppers = Vec::new();
    let mut boards = Vec::new();
    let mut offset = 0;
    let startup: anyhow::Result<()> = (|| {
        for (plugin, settings) in plugins {
            let name = plugin.name();
            let slots = board.slice(offset..offset + settings.processes);
            offset += settings.processes;
            let hooks = PoolHooks {
                max_requests: settings.max_requests,
                on_boot_failure: Arc::new({
                    let failed = boot_failed.clone();
                    move || {
                        failed.store(true, Ordering::Release);
                    }
                }),
            };
            let pool = Rapira::start_pool(
                &module,
                name,
                settings.mode,
                settings.entrypoint,
                slots,
                hooks,
                plugin.php().dispatcher,
            )?;
            let stopper = Stopper::default();
            let sink = pool.sink();
            pools.push(pool);
            running.push(run_plugin(
                plugin,
                sink,
                stopper.clone(),
                supervisor.process_control_timeout,
                supervisor.drain_grace(),
            )?);
            stoppers.push(stopper);
            boards.push((name, slots));
        }
        Ok(())
    })();
    while startup.is_ok() {
        if console::ASKED.load(Ordering::Acquire)
            || boot_failed.load(Ordering::Acquire)
            || running.iter().any(|plugin| plugin.is_finished())
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    tracing::info!(target: "rapira", "shutdown requested; draining plugins");
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
    let mut failed = false;
    for plugin in running {
        if let Err(error) = plugin.join() {
            tracing::error!(target: "rapira", "{error:#}");
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
    for (name, board) in boards {
        for slot in board.snapshot_slots() {
            tracing::info!(target: "rapira", pool = name, "slot {} pid {} state {} handled {} errors {} recycles {}", slot.id, slot.pid, slot.state, slot.handled, slot.errors, slot.recycles);
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

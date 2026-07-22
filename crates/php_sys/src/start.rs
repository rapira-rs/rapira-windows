use log::{error, info, trace};
use std::marker::PhantomData;
use std::os::raw::c_int;
use std::ptr::null_mut;
use std::sync::{Arc, Mutex};
use std::thread;
use std::thread::JoinHandle;
use tokio::sync::mpsc::{self, Receiver, Sender};

use crate::rapira_worker::{WorkerExit, rapira_worker};
use crate::{types::Job, types::Mode, *};

pub(crate) type JobRx = Arc<Mutex<Receiver<Job>>>;

/// One worker thread's PHP thread-state. ZTS-only: `ts_resource` allocates this thread's
/// per-thread globals (ctors incl. zend_call_stack_init), `ts_free_thread` releases them, and
/// the Windows TSRMLS cache is primed before any SG()/EG()/PG()/CG() access.
struct PhpThread;

impl PhpThread {
    fn new() -> Self {
        unsafe {
            ts_resource_ex(0, null_mut());
            rapira_tsrmls_cache_update();
        }
        Self
    }
}

impl Drop for PhpThread {
    fn drop(&mut self) {
        unsafe {
            ts_free_thread();
        }
    }
}

pub struct Rapira {
    pub(crate) intake: Option<Sender<Job>>,
    workers: Vec<JoinHandle<()>>,
    // !Send + !Sync: dropping from a foreign thread would run php_module_shutdown off the wrong
    // OS thread (UB).
    _not_send: PhantomData<*const ()>,
}

impl Rapira {
    pub fn start(mode: Mode, req_threads: usize) -> anyhow::Result<Self> {
        let num_threads: usize = req_threads.max(1);
        info!(target: "rapira", "booting worker mode, threads: {num_threads}");

        let mut module: sapi_module_struct = module::build_sapi_module();
        let started: bool = unsafe {
            php_tsrm_startup_ex(num_threads as c_int);
            rapira_tsrmls_cache_update();
            rapira_process_init();
            sapi_startup(&mut module);
            module
                .startup
                .is_some_and(|start| start(&mut module) == SUCCESS)
        };

        if !started {
            error!(target: "rapira", "php_module_startup failed, shutting down");
            unsafe {
                php_module_shutdown();
                sapi_shutdown();
                tsrm_shutdown();
            }
            return Err(anyhow::anyhow!("php_module_startup failed"));
        }

        let (intake, intake_rx) = mpsc::channel::<Job>(1024);
        let rx: JobRx = Arc::new(Mutex::new(intake_rx));

        let workers: Vec<JoinHandle<()>> = (0..num_threads)
            .map(|_| {
                let (rx, mode) = (rx.clone(), mode.clone());
                trace!(target: "rapira", "spawning worker thread");
                thread::spawn(move || worker_main(mode, rx))
            })
            .collect();

        Ok(Self {
            intake: Some(intake),
            workers,
            _not_send: PhantomData,
        })
    }
}

impl Drop for Rapira {
    fn drop(&mut self) {
        info!(target: "rapira", "shutting down, dropping");
        self.intake = None;
        let workers: Vec<JoinHandle<()>> = std::mem::take(&mut self.workers);

        // A worker may never come back: a leaked RapiraHandle keeps the intake open, parking
        // workers in pull_job. Bound the wait and, if a worker is still running, skip the C
        // teardown — php_module_shutdown on a live PHP thread is UB — and let process exit
        // reclaim it.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline && workers.iter().any(|w| !w.is_finished()) {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        if workers.iter().any(|w| !w.is_finished()) {
            error!(
                target: "rapira",
                "worker still running after grace; skipping PHP module shutdown to avoid UB on a live thread"
            );
            return;
        }

        for w in workers {
            let _ = w.join();
        }
        unsafe {
            php_module_shutdown();
            sapi_shutdown();
            tsrm_shutdown();
        }
    }
}

fn worker_main(mode: Mode, rx: JobRx) {
    loop {
        let php: PhpThread = PhpThread::new();
        let exit: WorkerExit = match &mode {
            Mode::Worker(script) => rapira_worker(script.clone(), rx.clone()),
        };
        drop(php); // ts_free_thread — per-thread globals dtor'd, TLS cache cleared
        if matches!(exit, WorkerExit::Closed) {
            break;
        }
        // Restart: the next PhpThread::new() re-runs ts_resource on this same OS thread —
        // fresh per-thread globals, ctors incl. zend_call_stack_init.
    }
}

/// Block for the next job (shutdown-aware): `None` means the intake channel closed — every
/// `Sender`/`RapiraHandle` was dropped, i.e. Rapira is shutting down. The single place the
/// shared receiver is consumed.
pub(crate) fn pull_job(rx: &JobRx) -> Option<Job> {
    // A poisoned lock is a previous panic, not a closed channel — recover the receiver so
    // worker exit stays tied to channel closure.
    let mut guard = rx.lock().unwrap_or_else(|poisoned| {
        error!(target: "rapira", "worker channel lock poisoned; recovering");
        poisoned.into_inner()
    });
    guard.blocking_recv()
}

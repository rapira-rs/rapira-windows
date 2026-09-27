use std::cell::RefCell;
use std::ffi::CString;
use std::marker::PhantomData;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::ptr::null_mut;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, TryRecvError, bounded};
use tracing::{error, info};

use crate::plugin::{Mode, PhpPart};
use crate::quota::{self, PoolHooks};
use crate::rapira_worker::{WorkerExit, rapira_worker};
use crate::scoreboard::{Event, sb_set, sb_update};
use crate::work::{DispatcherClasses, Sink, Work};
use crate::*;

thread_local! {
    static JOB_RX: RefCell<Option<JobRx>> = const { RefCell::new(None) };
    static PARTS: RefCell<Vec<PhpPart>> = const { RefCell::new(Vec::new()) };
}

#[derive(Clone)]
struct JobRx {
    rx: Receiver<Box<dyn Work>>,
    pending: Arc<AtomicUsize>,
    stop: Receiver<()>,
    stopping: Arc<AtomicBool>,
    handled: Arc<AtomicBool>,
}

pub struct PhpModule {
    active: Arc<AtomicUsize>,
    // PHP module teardown belongs to the boot thread.
    _not_send: PhantomData<*const ()>,
}

impl Drop for PhpModule {
    fn drop(&mut self) {
        if self.active.load(Ordering::Acquire) != 0 {
            error!(target: "rapira", "PHP threads are still alive; forcing process exit");
            unsafe {
                use windows_sys::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
                TerminateProcess(GetCurrentProcess(), 1);
            }
            std::process::abort();
        }
        unsafe {
            php_module_shutdown();
            sapi_shutdown();
            tsrm_shutdown();
        }
    }
}

struct ActiveThread(Arc<AtomicUsize>);
impl ActiveThread {
    fn new(active: &Arc<AtomicUsize>) -> Self {
        active.fetch_add(1, Ordering::Release);
        Self(active.clone())
    }
}
impl Drop for ActiveThread {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Release);
    }
}

struct PhpThread;
impl PhpThread {
    fn new(mode: Mode, directory: &CString) -> anyhow::Result<Self> {
        crate::context::reset_directory();
        let mode = match mode {
            Mode::Classic => RAPIRA_MODE_CLASSIC,
            Mode::Worker => RAPIRA_MODE_WORKER,
            Mode::Dispatcher => RAPIRA_MODE_DISPATCHER,
        };
        unsafe {
            ts_resource_ex(0, null_mut());
            rapira_tsrmls_cache_update();
        }
        let php = Self;
        anyhow::ensure!(
            unsafe { rapira_thread_init(mode as _, directory.as_ptr()) } == 0,
            "cannot enter the entrypoint directory: {}",
            directory.to_string_lossy()
        );
        unsafe { rapira_init_call_stack() };
        Ok(php)
    }
}
impl Drop for PhpThread {
    fn drop(&mut self) {
        unsafe {
            rapira_thread_disarm();
            ts_free_thread();
        }
    }
}

fn php_series(id: u32) -> (u32, u32) {
    (id / 10_000, (id / 100) % 100)
}

/// Registers the base classes and every linked plugin once before the pools start.
pub fn boot_master(parts: &[PhpPart], threads: usize) -> anyhow::Result<PhpModule> {
    let (headers, linked) = unsafe { (rapira_headers_php_version_id(), php_version_id()) };
    anyhow::ensure!(
        php_series(headers) == php_series(linked),
        "PHP runtime version {linked} does not match the build headers {headers}"
    );
    PARTS.set(parts.to_vec());
    let mut sapi = module::build_sapi_module();
    let started = unsafe {
        anyhow::ensure!(
            php_tsrm_startup_ex((threads + 1) as _),
            "TSRM startup failed"
        );
        rapira_tsrmls_cache_update();
        rapira_process_init();
        sapi_startup(&mut sapi);
        php_module_startup(&mut sapi, &raw mut rapira_module_entry) == SUCCESS
    };
    let module = PhpModule {
        active: Arc::new(AtomicUsize::new(0)),
        _not_send: PhantomData,
    };
    anyhow::ensure!(started, "php_module_startup failed");
    Ok(module)
}

#[unsafe(no_mangle)]
pub extern "C" fn rapira_rs_register_plugin_classes() {
    PARTS.with_borrow(|parts| {
        for part in parts {
            unsafe { (part.register)() };
        }
    });
}

pub struct Rapira {
    sink: Option<Sink>,
    workers: Vec<JoinHandle<()>>,
    stopping: Arc<AtomicBool>,
    stop_tx: Option<Sender<()>>,
}

impl Rapira {
    pub fn start_pool(
        module: &PhpModule,
        name: &'static str,
        mode: Mode,
        entrypoint: PathBuf,
        board: rapira_scoreboard::Scoreboard,
        hooks: PoolHooks,
        classes: DispatcherClasses,
    ) -> anyhow::Result<Self> {
        let directory = CString::new(
            entrypoint
                .parent()
                .expect("validated entrypoint")
                .to_string_lossy()
                .as_bytes(),
        )?;
        let pending = Arc::new(AtomicUsize::new(0));
        let (tx, rx) = bounded(1024);
        let (stop_tx, stop) = bounded(0);
        let stopping = Arc::new(AtomicBool::new(false));
        let job = JobRx {
            rx,
            pending: pending.clone(),
            stop,
            stopping: stopping.clone(),
            handled: Arc::new(AtomicBool::new(false)),
        };
        let reported = Arc::new(AtomicBool::new(false));
        let (gate_tx, gate_rx) = bounded(board.nslots());
        let mut pool = Self {
            sink: Some(Sink::new(tx, pending)),
            workers: Vec::with_capacity(board.nslots()),
            stopping,
            stop_tx: Some(stop_tx),
        };
        for index in 0..board.nslots() {
            board.set_starting(index);
            let slot = board.slot(index);
            let (job, hooks, reported, gate) = (
                job.clone(),
                hooks.clone(),
                reported.clone(),
                gate_rx.clone(),
            );
            let (entrypoint, directory) = (entrypoint.clone(), directory.clone());
            let active = ActiveThread::new(&module.active);
            let worker = thread::Builder::new()
                .name(format!("rapira-{name}-{index}"))
                .spawn(move || {
                    let _active = active;
                    if gate.recv().is_ok() {
                        worker_main(
                            name, mode, entrypoint, directory, job, slot, index, hooks, reported,
                            classes,
                        );
                    }
                });
            match worker {
                Ok(worker) => pool.workers.push(worker),
                Err(error) => {
                    drop(gate_tx);
                    return Err(error.into());
                }
            }
        }
        for _ in 0..board.nslots() {
            gate_tx.send(()).expect("worker start gate is connected");
        }
        Ok(pool)
    }

    pub fn sink(&self) -> Sink {
        self.sink.as_ref().expect("active pool").clone()
    }

    pub fn stop(&mut self) {
        self.stopping.store(true, Ordering::Release);
        self.stop_tx = None;
        self.sink = None;
    }

    pub fn join(&mut self, deadline: Instant) -> bool {
        self.stop();
        while Instant::now() < deadline && self.workers.iter().any(|w| !w.is_finished()) {
            thread::sleep(Duration::from_millis(20));
        }
        if self.workers.iter().any(|w| !w.is_finished()) {
            return false;
        }
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
        true
    }
}

impl Drop for Rapira {
    fn drop(&mut self) {
        self.join(Instant::now() + Duration::from_secs(5));
    }
}

fn effective_quota(max: u64, index: usize) -> u64 {
    if max == 0 {
        return 0;
    }
    use std::hash::{BuildHasher, Hasher};
    let mut hash = std::collections::hash_map::RandomState::new().build_hasher();
    hash.write_usize(index);
    max.saturating_add(1 + hash.finish() % (max / 2).max(1))
}

#[allow(clippy::too_many_arguments)]
fn worker_main(
    name: &str,
    mode: Mode,
    entrypoint: PathBuf,
    directory: CString,
    job: JobRx,
    slot: &'static rapira_scoreboard::SharedSlot,
    index: usize,
    hooks: PoolHooks,
    reported: Arc<AtomicBool>,
    classes: DispatcherClasses,
) {
    slot.bind(std::process::id());
    crate::context::set_script(&entrypoint);
    JOB_RX.set(Some(job.clone()));
    crate::exchange::set_classes(classes);
    let mut streak: u32 = 0;
    loop {
        let started = Instant::now();
        let php = match PhpThread::new(mode, &directory) {
            Ok(php) => php,
            Err(error) => {
                error!(target: "rapira", "pool {name} thread {index}: {error:#}");
                (hooks.on_boot_failure)();
                break;
            }
        };
        crate::exchange::forget_dispatcher();
        crate::exchange::cycle_reset();
        sb_set(slot);
        quota::install(effective_quota(hooks.max_requests, index));
        info!(target: "rapira", pool = name, "worker thread {index} ready");
        let outcome = catch_unwind(AssertUnwindSafe(|| match mode {
            Mode::Classic => {
                classic_worker::classic_worker();
                WorkerExit::Closed
            }
            Mode::Worker | Mode::Dispatcher => rapira_worker(entrypoint.clone()),
        }));
        if outcome.is_err() {
            error!(target: "rapira", "pool {name} thread {index} panicked");
            crate::exchange::reclaim_current();
            crate::context::unbind_server_context();
            quota::fire_unhealthy();
        }
        let unhealthy = quota::is_unhealthy();
        drop(php);
        if job.stopping.load(Ordering::Acquire)
            || (matches!(outcome, Ok(WorkerExit::Closed)) && !quota::is_draining())
        {
            break;
        }
        if unhealthy {
            if !job.handled.load(Ordering::Acquire) && !reported.swap(true, Ordering::AcqRel) {
                (hooks.on_boot_failure)();
            }
            if started.elapsed() >= Duration::from_secs(10) {
                streak = 0;
            }
            let delay = Duration::from_millis(100) * (1 << streak.min(8));
            streak = streak.saturating_add(1);
            if !matches!(job.stop.recv_timeout(delay), Err(RecvTimeoutError::Timeout)) {
                break;
            }
        } else {
            streak = 0;
        }
        sb_update(Event::Recycled);
        info!(target: "rapira", pool = name, "worker thread {index} recycling");
    }
    JOB_RX.set(None);
}

pub(crate) fn note_handled() {
    JOB_RX.with_borrow(|slot| {
        if let Some(job) = slot {
            job.handled.store(true, Ordering::Release);
        }
    });
}

pub(crate) fn pull_job() -> Option<Box<dyn Work>> {
    match pull_job_wait(None) {
        Pulled::Job(job) => Some(job),
        _ => None,
    }
}

pub(crate) enum Pulled {
    Job(Box<dyn Work>),
    Timeout,
    Empty,
    Closed,
}

pub(crate) fn pull_job_wait(timeout: Option<Duration>) -> Pulled {
    if quota::is_draining() {
        sb_update(Event::Idle);
        return Pulled::Closed;
    }
    JOB_RX.with_borrow(|slot| {
        let Some(job) = slot else {
            return Pulled::Closed;
        };
        if job.stopping.load(Ordering::Acquire) {
            return Pulled::Closed;
        }
        sb_update(Event::Idle);
        let got = match timeout {
            None => crossbeam_channel::select_biased! {
                recv(job.stop) -> _ => Err(RecvTimeoutError::Disconnected),
                recv(job.rx) -> unit => unit.map_err(|_| RecvTimeoutError::Disconnected),
            },
            Some(timeout) => crossbeam_channel::select_biased! {
                recv(job.stop) -> _ => Err(RecvTimeoutError::Disconnected),
                recv(job.rx) -> unit => unit.map_err(|_| RecvTimeoutError::Disconnected),
                default(timeout) => Err(RecvTimeoutError::Timeout),
            },
        };
        sb_update(Event::Active);
        match got {
            Ok(unit) => {
                job.pending.fetch_sub(1, Ordering::Relaxed);
                Pulled::Job(unit)
            }
            Err(RecvTimeoutError::Timeout) => Pulled::Timeout,
            Err(RecvTimeoutError::Disconnected) => Pulled::Closed,
        }
    })
}

pub(crate) fn pull_job_try() -> Pulled {
    if quota::is_draining() {
        sb_update(Event::Idle);
        return Pulled::Closed;
    }
    JOB_RX.with_borrow(|slot| {
        let Some(job) = slot else {
            return Pulled::Closed;
        };
        if job.stopping.load(Ordering::Acquire) {
            return Pulled::Closed;
        }
        sb_update(Event::Idle);
        let got = job.rx.try_recv();
        sb_update(Event::Active);
        match got {
            Ok(unit) => {
                job.pending.fetch_sub(1, Ordering::Relaxed);
                Pulled::Job(unit)
            }
            Err(TryRecvError::Empty) => Pulled::Empty,
            Err(TryRecvError::Disconnected) => Pulled::Closed,
        }
    })
}

pub(crate) fn pending_depth() -> usize {
    JOB_RX.with_borrow(|slot| {
        slot.as_ref()
            .map_or(0, |job| job.pending.load(Ordering::Relaxed))
    })
}

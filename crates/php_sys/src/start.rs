use std::cell::RefCell;
use std::marker::PhantomData;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::null_mut;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, TryRecvError, bounded};
use tracing::{error, info, trace};
use types::Context;

use crate::quota::{self, PoolHooks};
use crate::rapira_worker::{WorkerExit, rapira_worker};
use crate::scoreboard::{Event, ScoreboardSnapshot, sb_set, sb_update};
use crate::{classic_worker::classic_worker, types::Mode, *};

const QUICK_CRASH: Duration = Duration::from_secs(10);
const RESPAWN_BASE: Duration = Duration::from_millis(100);
const JOIN_GRACE: Duration = Duration::from_secs(5);

thread_local! {
    static JOB_RX: RefCell<Option<JobRx>> = const { RefCell::new(None) };
}

pub(crate) struct Intake {
    pub(crate) tx: Sender<Context>,
    pub(crate) pending: Arc<AtomicUsize>,
}

#[derive(Clone)]
struct JobRx {
    rx: Receiver<Context>,
    pending: Arc<AtomicUsize>,
    stop: Receiver<()>,
    stopping: Arc<AtomicBool>,
    handled: Arc<AtomicBool>,
}

struct PhpThread;

impl PhpThread {
    fn new() -> Self {
        unsafe {
            ts_resource_ex(0, null_mut());
            rapira_tsrmls_cache_update();
            rapira_thread_init();
        }
        Self
    }
}

impl Drop for PhpThread {
    fn drop(&mut self) {
        #[cfg(test)]
        let timer_probe = tests::before_thread_disarm();
        unsafe { rapira_thread_disarm() };
        #[cfg(test)]
        tests::after_thread_disarm(timer_probe);
        unsafe { ts_free_thread() };
    }
}

struct PhpModule;

impl Drop for PhpModule {
    fn drop(&mut self) {
        unsafe {
            php_module_shutdown();
            sapi_shutdown();
            tsrm_shutdown();
        }
    }
}

pub struct Rapira {
    pub(crate) intake: Option<Intake>,
    pub(crate) superglobals: bool,
    pub(crate) dispatcher: bool,
    workers: Vec<JoinHandle<()>>,
    board: rapira_scoreboard::Scoreboard,
    module: Option<PhpModule>,
    stopping: Arc<AtomicBool>,
    stop_tx: Option<Sender<()>>,
    // PHP module teardown must run on the boot thread.
    _not_send: PhantomData<*const ()>,
}

/// Splits a `PHP_VERSION_ID` (major * 10000 + minor * 100 + patch) into major and minor: https://www.php.net/manual/en/function.phpversion.php
fn php_series(id: u32) -> (u32, u32) {
    (id / 10_000, (id / 100) % 100)
}

/// bindgen binds Zend structures at build time. A libphp from another PHP minor has a different ABI because `sapi_startup` receives a structure with a different layout.
fn check_linked_php() -> anyhow::Result<()> {
    // SAFETY: Both accessors read a compile-time constant and do not access engine state, so they are valid before startup.
    let (headers, linked) = unsafe { (rapira_headers_php_version_id(), php_version_id()) };
    let (want, got) = (php_series(headers), php_series(linked));
    anyhow::ensure!(
        want == got,
        "linked libphp is PHP {}.{}, but this rapira was built against PHP {}.{}. \
         Use a libphp from the same PHP minor as the build.",
        got.0,
        got.1,
        want.0,
        want.1
    );
    Ok(())
}

impl Rapira {
    pub fn start(mode: Mode) -> anyhow::Result<Self> {
        Self::start_pool(mode, 1, PoolHooks::default())
    }

    pub fn start_pool(mode: Mode, processes: usize, hooks: PoolHooks) -> anyhow::Result<Self> {
        check_linked_php()?;
        let board = rapira_scoreboard::Scoreboard::create(processes)?;
        info!(target: "rapira", "booting with mode: {mode:?}, threads: {processes}");
        let mut module: _sapi_module_struct = module::build_sapi_module();
        let started: bool = unsafe {
            php_tsrm_startup_ex((processes + 1) as c_int);
            rapira_tsrmls_cache_update();
            rapira_process_init();
            sapi_startup(&mut module);
            module
                .startup
                .is_some_and(|start| start(&mut module) == SUCCESS)
        };
        let module = PhpModule;
        if !started {
            error!(target: "rapira", "php_module_startup failed, shutting down");
            drop(module);
            return Err(anyhow::anyhow!("php_module_startup failed"));
        }

        let superglobals = !matches!(mode, Mode::Dispatcher(_));
        let dispatcher = matches!(mode, Mode::Dispatcher(_));
        // SAFETY: safe, trust me, I'm a developer
        unsafe {
            crate::rapira_mode = match &mode {
                Mode::Classic => RAPIRA_MODE_CLASSIC,
                Mode::Worker(_) => RAPIRA_MODE_WORKER,
                Mode::Dispatcher(_) => RAPIRA_MODE_DISPATCHER,
            } as c_int;
        }

        let pending = Arc::new(AtomicUsize::new(0));
        let (intake_tx, intake_rx) = bounded::<Context>(1024);
        let (stop_tx, stop_rx) = bounded(0);
        let stopping = Arc::new(AtomicBool::new(false));
        let job_rx = JobRx {
            rx: intake_rx,
            pending: pending.clone(),
            stop: stop_rx,
            stopping: stopping.clone(),
            handled: Arc::new(AtomicBool::new(false)),
        };
        let reported_boot_failure = Arc::new(AtomicBool::new(false));
        let (start_tx, start_rx) = bounded(processes);
        let mut rapira = Self {
            intake: Some(Intake {
                tx: intake_tx,
                pending,
            }),
            superglobals,
            dispatcher,
            workers: Vec::with_capacity(processes),
            board,
            module: Some(module),
            stopping,
            stop_tx: Some(stop_tx),
            _not_send: PhantomData,
        };

        for index in 0..processes {
            let slot = board.slot(index).expect("worker slot exists");
            board.set_starting(index);
            let rx = job_rx.clone();
            let mode = mode.clone();
            let hooks = hooks.clone();
            let reported = reported_boot_failure.clone();
            let start = start_rx.clone();
            trace!(target: "rapira", "spawning worker thread {index}");
            let worker = thread::Builder::new()
                .name(format!("rapira-worker-{index}"))
                .spawn(move || {
                    if start.recv().is_ok() {
                        worker_main(mode, rx, slot, index, hooks, reported);
                    }
                });
            match worker {
                Ok(worker) => rapira.workers.push(worker),
                Err(error) => {
                    rapira.stopping.store(true, Ordering::Release);
                    drop(start_tx);
                    for worker in rapira.workers.drain(..) {
                        let _ = worker.join();
                    }
                    return Err(error.into());
                }
            }
        }
        for _ in 0..processes {
            start_tx
                .send(())
                .expect("worker start gate remains connected");
        }
        drop(start_tx);
        Ok(rapira)
    }

    pub fn shutdown(mut self) -> bool {
        self.stop_and_join()
    }

    pub fn scoreboard(&self) -> ScoreboardSnapshot {
        crate::scoreboard::snapshot(&self.board)
    }

    fn stop_and_join(&mut self) -> bool {
        if self.module.is_none() {
            return true;
        }
        info!(target: "rapira", "stopping worker threads");
        self.stopping.store(true, Ordering::Release);
        self.stop_tx = None;
        self.intake = None;
        let workers = std::mem::take(&mut self.workers);
        let deadline = Instant::now() + JOIN_GRACE;
        while Instant::now() < deadline && workers.iter().any(|worker| !worker.is_finished()) {
            thread::sleep(Duration::from_millis(20));
        }
        if workers.iter().any(|worker| !worker.is_finished()) {
            error!(target: "rapira", "worker thread still running after grace; PHP module remains active");
            std::mem::forget(self.module.take());
            return false;
        }
        for worker in workers {
            let _ = worker.join();
        }
        drop(self.module.take());
        true
    }
}

impl Drop for Rapira {
    fn drop(&mut self) {
        self.stop_and_join();
    }
}

fn next_backoff(streak: &mut u32, lived: Duration) -> Duration {
    if lived >= QUICK_CRASH {
        *streak = 0;
    }
    let delay = RESPAWN_BASE * (1 << (*streak).min(8));
    *streak = streak.saturating_add(1);
    delay
}

fn report_boot_failure(
    handled: &AtomicBool,
    reported: &AtomicBool,
    hook: &(dyn Fn() + Send + Sync),
) {
    if !handled.load(Ordering::Acquire) && !reported.swap(true, Ordering::AcqRel) {
        hook();
    }
}

/// Combines the worker thread index with time so interpreters do not recycle at the same request count.
fn effective_quota(max_requests: u64, thread_index: usize) -> u64 {
    if max_requests == 0 {
        return 0;
    }
    let grace = (max_requests / 2).max(1);
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_usize(thread_index);
    h.write_u128(
        std::time::UNIX_EPOCH
            .elapsed()
            .map(|d| d.as_nanos())
            .unwrap_or(0),
    );
    max_requests.saturating_add(1 + (h.finish() % grace))
}

fn worker_main(
    mode: Mode,
    rx: JobRx,
    slot: &'static rapira_scoreboard::SharedSlot,
    index: usize,
    hooks: PoolHooks,
    reported_boot_failure: Arc<AtomicBool>,
) {
    let stop = rx.stop.clone();
    let stopping = rx.stopping.clone();
    let handled = rx.handled.clone();
    slot.bind(index as u32);
    JOB_RX.with_borrow_mut(|slot| *slot = Some(rx));
    let mut crash_streak = 0;
    let mut first_generation = true;
    while first_generation || !stopping.load(Ordering::Acquire) {
        first_generation = false;
        let started = Instant::now();
        let php = PhpThread::new();
        crate::exchange::forget_dispatcher();
        crate::exchange::cycle_reset();
        sb_set(slot);
        quota::install(effective_quota(hooks.max_requests, index));
        sb_update(Event::Healthy);
        sb_update(Event::Idle);
        info!(target: "rapira", "worker thread {index} ready");
        let exit = catch_unwind(AssertUnwindSafe(|| match &mode {
            Mode::Classic => {
                classic_worker();
                WorkerExit::Closed
            }
            Mode::Worker(script) | Mode::Dispatcher(script) => rapira_worker(script.clone()),
        }));
        if exit.is_err() {
            error!(target: "rapira", "worker thread {index} panicked");
            crate::exchange::reclaim_current();
            // An interrupted classic job can leave borrowed request pointers in SG.
            crate::context::unbind_server_context();
            sb_update(Event::Unhealthy);
        }
        let unhealthy = quota::is_unhealthy();
        drop(php);
        if stopping.load(Ordering::Acquire)
            || (matches!(exit, Ok(WorkerExit::Closed)) && !quota::is_draining())
        {
            break;
        }
        if unhealthy {
            report_boot_failure(&handled, &reported_boot_failure, &*hooks.on_boot_failure);
            let delay = next_backoff(&mut crash_streak, started.elapsed());
            if !matches!(stop.recv_timeout(delay), Err(RecvTimeoutError::Timeout)) {
                break;
            }
        } else {
            crash_streak = 0;
        }
        sb_update(Event::Recycled);
        info!(target: "rapira", "worker thread {index} recycling");
    }
}

pub(crate) fn note_handled() {
    JOB_RX.with_borrow(|slot| {
        if let Some(rx) = slot {
            rx.handled.store(true, Ordering::Release);
        }
    });
}

pub(crate) fn pull_job() -> Option<Context> {
    match pull_job_wait(None) {
        Pulled::Job(job) => Some(*job),
        _ => None,
    }
}

pub(crate) enum Pulled {
    // Use a `Box` because a `Context` is approximately 600 bytes and the other variants are empty.
    Job(Box<Context>),
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
        let Some(job_r) = slot.as_ref() else {
            return Pulled::Closed;
        };
        if job_r.stopping.load(Ordering::Acquire) {
            return Pulled::Closed;
        }
        sb_update(Event::Idle);
        let got = match timeout {
            None => crossbeam_channel::select_biased! {
                recv(job_r.stop) -> _ => Err(RecvTimeoutError::Disconnected),
                recv(job_r.rx) -> job => job.map_err(|_| RecvTimeoutError::Disconnected),
            },
            Some(timeout) => crossbeam_channel::select_biased! {
                recv(job_r.stop) -> _ => Err(RecvTimeoutError::Disconnected),
                recv(job_r.rx) -> job => job.map_err(|_| RecvTimeoutError::Disconnected),
                default(timeout) => Err(RecvTimeoutError::Timeout),
            },
        };
        sb_update(Event::Active);
        match got {
            Ok(job) => {
                job_r.pending.fetch_sub(1, Ordering::Relaxed);
                Pulled::Job(Box::new(job))
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
        let Some(job_r) = slot.as_ref() else {
            return Pulled::Closed;
        };
        if job_r.stopping.load(Ordering::Acquire)
            || !matches!(job_r.stop.try_recv(), Err(TryRecvError::Empty))
        {
            return Pulled::Closed;
        }
        sb_update(Event::Idle);
        let got = job_r.rx.try_recv();
        sb_update(Event::Active);
        match got {
            Ok(job) => {
                job_r.pending.fetch_sub(1, Ordering::Relaxed);
                Pulled::Job(Box::new(job))
            }
            Err(TryRecvError::Empty) => Pulled::Empty,
            Err(TryRecvError::Disconnected) => Pulled::Closed,
        }
    })
}

pub(crate) fn pending_depth() -> usize {
    JOB_RX.with_borrow(|slot| {
        slot.as_ref()
            .map_or(0, |job_r| job_r.pending.load(Ordering::Relaxed))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use std::sync::mpsc;

    fn delegate_to_child(test: &str, child_env: &str) -> bool {
        if std::env::var_os(child_env).is_some() {
            return false;
        }
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", test, "--nocapture"])
            .env(child_env, "1")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while child.try_wait().unwrap().is_none() {
            if std::time::Instant::now() >= deadline {
                child.kill().unwrap();
                let output = child.wait_with_output().unwrap();
                panic!(
                    "PHP child timed out: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            thread::sleep(Duration::from_millis(20));
        }
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "PHP child failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        true
    }

    static TIMER_PROBE: AtomicUsize = AtomicUsize::new(0);
    const TIMER_REQUESTED: usize = 1;
    const TIMER_RUNNING: usize = 2;
    const TIMER_CANCELLED: usize = 3;
    const TIMER_FIRED: usize = 4;
    const TIMER_CONTROL_FAILED: usize = 5;

    unsafe extern "C" {
        fn zend_atomic_bool_load(value: *mut zend_atomic_bool) -> bool;
    }

    fn wait_for_timer() -> bool {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            if unsafe { zend_atomic_bool_load(&raw mut (*rapira_eg()).timed_out) } {
                return true;
            }
            if std::time::Instant::now() >= deadline {
                return false;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    pub(super) fn before_thread_disarm() -> bool {
        if TIMER_PROBE
            .compare_exchange(
                TIMER_REQUESTED,
                TIMER_RUNNING,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_err()
        {
            return false;
        }
        unsafe { rapira_timer_rearm(1) };
        let control_fired = wait_for_timer();
        unsafe { rapira_thread_disarm() };
        if !control_fired {
            TIMER_PROBE.store(TIMER_CONTROL_FAILED, Ordering::Release);
            return false;
        }
        unsafe { rapira_timer_rearm(1) };
        true
    }

    pub(super) fn after_thread_disarm(probe: bool) {
        if !probe {
            return;
        }
        let fired = wait_for_timer();
        unsafe { rapira_thread_disarm() };
        TIMER_PROBE.store(
            if fired { TIMER_FIRED } else { TIMER_CANCELLED },
            Ordering::Release,
        );
    }

    fn timer_probe_request(script: &std::path::Path) -> types::Request {
        types::Request {
            method: "GET".into(),
            uri: "/".into(),
            target: None,
            authority: None,
            https: false,
            query: String::new(),
            protocol: "HTTP/1.1".into(),
            remote: types::Addr::Inet("127.0.0.1:12345".parse().unwrap()),
            server: types::Addr::Inet("127.0.0.1:8080".parse().unwrap()),
            server_name: "localhost".into(),
            server_port: 8080,
            script_name: "/worker.php".into(),
            document_root: script.parent().unwrap().to_string_lossy().into_owned(),
            script_filename: script.into(),
            headers: Vec::new(),
            server_vars: Vec::new(),
            content_type: None,
            content_length: 0,
            body: types::Body::Raw(Box::new(std::io::empty())),
            received_at: None,
            tls: None,
        }
    }

    #[test]
    fn interpreter_teardown_disarms_an_armed_timer() {
        const CHILD: &str = "RAPIRA_TEST_TEARDOWN_TIMER_CHILD";
        if delegate_to_child(
            "start::tests::interpreter_teardown_disarms_an_armed_timer",
            CHILD,
        ) {
            return;
        }

        let script =
            std::env::temp_dir().join(format!("rapira-teardown-timer-{}.php", std::process::id()));
        std::fs::write(
            &script,
            r"<?php while (\Rapira\handle_request(static function (): void { echo 'ok'; })) {}",
        )
        .unwrap();
        TIMER_PROBE.store(TIMER_REQUESTED, Ordering::Release);
        let rapira = Rapira::start_pool(
            Mode::Worker(script.clone()),
            1,
            PoolHooks {
                max_requests: 1,
                ..PoolHooks::default()
            },
        )
        .unwrap();
        let handle = rapira.handle();
        for _ in 0..3 {
            let mut frames = handle
                .handle_blocking(timer_probe_request(&script))
                .unwrap();
            let mut body = Vec::new();
            let mut complete = false;
            while let Some(frame) = frames.blocking_recv() {
                match frame {
                    types::Frame::Head { head, .. } => assert_eq!(head.status, 200),
                    types::Frame::Chunk(chunk) => body.extend_from_slice(&chunk),
                    types::Frame::End { truncated, .. } => {
                        assert!(!truncated);
                        complete = true;
                        break;
                    }
                    _ => {}
                }
            }
            assert!(complete);
            assert_eq!(body, b"ok");
        }
        assert_eq!(TIMER_PROBE.load(Ordering::Acquire), TIMER_CANCELLED);
        assert!(rapira.shutdown());
        assert!(matches!(
            handle.handle_blocking(timer_probe_request(&script)),
            Err(HandleError::Stopped)
        ));

        std::fs::write(
            &script,
            r"<?php file_put_contents(__FILE__ . '.booted', 'ready'); while (\Rapira\handle_request(static function (): void {})) {}",
        )
        .unwrap();
        let immediate = Rapira::start(Mode::Worker(script.clone())).unwrap();
        assert!(immediate.shutdown());
        let booted = script.with_extension("php.booted");
        assert_eq!(std::fs::read_to_string(&booted).unwrap(), "ready");
        std::fs::remove_file(booted).unwrap();
        std::fs::remove_file(script).unwrap();
    }

    #[test]
    fn response_channel_wait_keeps_the_wall_timer_armed() {
        const CHILD: &str = "RAPIRA_TEST_RESPONSE_TIMER_CHILD";
        const RESPONSE_FRAME_CAP: usize = 4;
        if delegate_to_child(
            "start::tests::response_channel_wait_keeps_the_wall_timer_armed",
            CHILD,
        ) {
            return;
        }

        let script =
            std::env::temp_dir().join(format!("rapira-response-timer-{}.php", std::process::id()));
        std::fs::write(
            &script,
            r"<?php
$dispatcher = \Rapira\get_dispatcher();
try {
    while (true) {
        $exchange = $dispatcher->receive();
        $exchange->writeHead(200);
        $exchange->writeBody('one', eos: false);
        $exchange->writeBody('two', eos: false);
        $exchange->writeBody('three', eos: false);
        set_time_limit(1);
        file_put_contents(__FILE__ . '.blocking', 'ready');
        $exchange->writeBody('four', eos: false);
        file_put_contents(__FILE__ . '.survived', 'bad');
        $exchange->writeBody('', eos: true);
    }
} catch (\Rapira\Exception\ClosedException) {
}",
        )
        .unwrap();
        let blocking = script.with_extension("php.blocking");
        let survived = script.with_extension("php.survived");
        let rapira = Rapira::start(Mode::Dispatcher(script.clone())).unwrap();
        let handle = rapira.handle();
        let mut frames = handle
            .handle_blocking(timer_probe_request(&script))
            .unwrap();

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while (!blocking.exists() || frames.len() < RESPONSE_FRAME_CAP)
            && std::time::Instant::now() < deadline
        {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(
            blocking.exists(),
            "the dispatcher must arm the timer before the blocked write"
        );
        assert_eq!(
            frames.len(),
            RESPONSE_FRAME_CAP,
            "the response channel must become full"
        );
        thread::sleep(Duration::from_secs(2));
        assert!(
            !survived.exists(),
            "the dispatcher must remain blocked before capacity is released"
        );

        assert!(matches!(frames.try_recv(), Ok(types::Frame::Head { .. })));
        let mut sent_blocked_chunk = false;
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            match frames.try_recv() {
                Ok(types::Frame::Chunk(chunk)) if chunk == b"four"[..] => {
                    sent_blocked_chunk = true;
                }
                Ok(_) => {}
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "the timed unit must close its response channel"
                    );
                    thread::sleep(Duration::from_millis(10));
                }
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => break,
            }
        }
        assert!(
            sent_blocked_chunk,
            "the blocked response write must finish after capacity is released"
        );
        assert!(
            !survived.exists(),
            "PHP must stop before it executes the next opcode"
        );

        assert!(rapira.shutdown());
        std::fs::remove_file(blocking).unwrap();
        std::fs::remove_file(script).unwrap();
    }

    fn test_intake() -> (
        crossbeam_channel::Sender<Context>,
        crossbeam_channel::Sender<()>,
        JobRx,
    ) {
        let (tx, rx) = crossbeam_channel::bounded(1);
        let (stop_tx, stop) = crossbeam_channel::bounded(0);
        (
            tx,
            stop_tx,
            JobRx {
                rx,
                pending: Arc::new(AtomicUsize::new(0)),
                stop,
                stopping: Arc::new(AtomicBool::new(false)),
                handled: Arc::new(AtomicBool::new(false)),
            },
        )
    }

    #[test]
    fn backoff_progression_and_cap() {
        for (mut streak, millis) in [(0, 100), (1, 200), (2, 400), (8, 25_600), (100, 25_600)] {
            assert_eq!(
                next_backoff(&mut streak, Duration::ZERO),
                Duration::from_millis(millis)
            );
        }
    }

    #[test]
    fn ten_second_generation_resets_the_crash_streak() {
        let mut streak = 5;
        assert_eq!(
            next_backoff(&mut streak, Duration::from_millis(9_999)),
            Duration::from_millis(3_200)
        );
        assert_eq!(streak, 6);
        assert_eq!(
            next_backoff(&mut streak, Duration::from_secs(10)),
            Duration::from_millis(100)
        );
        assert_eq!(streak, 1);
    }

    #[test]
    fn effective_quota_keeps_core_bounds() {
        for index in 0..16 {
            assert_eq!(effective_quota(0, index), 0);
            assert_eq!(effective_quota(1, index), 2);
            assert_eq!(effective_quota(2, index), 3);
            assert!((101..=150).contains(&effective_quota(100, index)));
            assert_eq!(effective_quota(u64::MAX, index), u64::MAX);
        }
    }

    #[test]
    fn stop_interrupts_idle_receive_with_intake_open() {
        let (_intake, stop, rx) = test_intake();
        let (ready_tx, ready_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            JOB_RX.with_borrow_mut(|slot| *slot = Some(rx));
            ready_tx.send(()).unwrap();
            done_tx
                .send(matches!(pull_job_wait(None), Pulled::Closed))
                .unwrap();
        });
        ready_rx.recv().unwrap();
        drop(stop);
        assert!(done_rx.recv_timeout(Duration::from_secs(1)).unwrap());
        worker.join().unwrap();
    }

    #[test]
    fn generation_drain_closes_receives_until_reset() {
        let (_intake, _stop, rx) = test_intake();
        JOB_RX.with_borrow_mut(|slot| *slot = Some(rx));
        quota::install(1);
        quota::tick();
        assert!(matches!(pull_job_wait(None), Pulled::Closed));
        assert!(matches!(pull_job_try(), Pulled::Closed));

        quota::install(0);
        assert!(matches!(pull_job_try(), Pulled::Empty));
        assert!(matches!(
            pull_job_wait(Some(Duration::ZERO)),
            Pulled::Timeout
        ));
    }

    #[test]
    fn shed_requests_do_not_disable_the_boot_failure_hook() {
        let (_intake, _stop, rx) = test_intake();
        let handled = rx.handled.clone();
        JOB_RX.with_borrow_mut(|slot| *slot = Some(rx));
        let board = rapira_scoreboard::Scoreboard::create(1).unwrap();
        let slot = board.slot(0).unwrap();
        slot.bind(0);
        sb_set(slot);
        quota::install(0);
        for _ in 0..4 {
            sb_update(Event::Shed);
        }
        assert_eq!(board.snapshot_slots()[0].handled, 4);
        let reported = AtomicBool::new(false);
        let calls = AtomicUsize::new(0);
        let hook = || {
            calls.fetch_add(1, Ordering::Relaxed);
        };
        report_boot_failure(&handled, &reported, &hook);
        report_boot_failure(&handled, &reported, &hook);
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn handled_request_disables_boot_failure_with_unlimited_quota() {
        let (_intake, _stop, rx) = test_intake();
        let handled = rx.handled.clone();
        JOB_RX.with_borrow_mut(|slot| *slot = Some(rx));
        let board = rapira_scoreboard::Scoreboard::create(1).unwrap();
        let slot = board.slot(0).unwrap();
        slot.bind(0);
        sb_set(slot);
        quota::install(0);
        sb_update(Event::Handled(false));
        quota::install(0);
        let reported = AtomicBool::new(false);
        report_boot_failure(&handled, &reported, &|| panic!("request was handled"));
        assert!(!reported.load(Ordering::Relaxed));
    }

    #[test]
    fn php_series_drops_the_patch() {
        assert_eq!(php_series(80_508), (8, 5));
        assert_eq!(php_series(80_426), (8, 4));
        assert_eq!(php_series(80_500), php_series(80_599));
        assert_ne!(php_series(80_400), php_series(80_500));
    }
}

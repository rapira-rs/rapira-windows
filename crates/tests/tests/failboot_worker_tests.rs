use php_sys::{Mode, PoolHooks, Rapira};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;
use tests::{drain, fixture, php_lock, req};

// A worker that has a fatal error before its receive loop must return 503 for the queued job. `Drop` must return without waiting for repeated startup attempts.
#[test]
fn failboot_worker_serves_503_and_drops_cleanly() -> anyhow::Result<()> {
    let _guard = php_lock();
    let (done_tx, done_rx) = mpsc::sync_channel::<(u16, String)>(1);

    let scenario = std::thread::spawn(move || -> anyhow::Result<()> {
        let r = Rapira::start(Mode::Dispatcher(fixture(
            "failboot_worker_tests/failboot-worker.php",
        )))?;
        let h = r.handle();
        let rx = h.handle_blocking(req("/", "failboot_worker_tests/failboot-worker.php"))?;
        drop(h);
        let (status, body) = drain(rx);
        drop(r);
        let _ = done_tx.send((status, body));
        Ok(())
    });

    let (status, _body) = done_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("broken worker black-holed the request or hung Drop (A6 regression)");
    assert_eq!(status, 503, "a boot-failed worker must 503 the queued job");
    scenario.join().expect("scenario thread panicked")?;
    Ok(())
}

// UNHEALTHY_AFTER, which is 5, consecutive startup failures must set the unhealthy scoreboard flag. Each failed start must return 503 for its queued job.
#[test]
fn failboot_worker_flags_unhealthy_after_threshold() -> anyhow::Result<()> {
    let _guard = php_lock();
    let (done_tx, done_rx) = mpsc::sync_channel::<(usize, Vec<u16>)>(1);

    let scenario = std::thread::spawn(move || -> anyhow::Result<()> {
        let (hook_entered_tx, hook_entered_rx) = mpsc::sync_channel::<()>(1);
        let (hook_release_tx, hook_release_rx) = mpsc::sync_channel::<()>(1);
        let hook_release_rx = Arc::new(Mutex::new(hook_release_rx));
        let r = Rapira::start_pool(
            Mode::Dispatcher(fixture("failboot_worker_tests/failboot-worker.php")),
            1,
            PoolHooks {
                on_boot_failure: Arc::new(move || {
                    let _ = hook_entered_tx.send(());
                    let _ = hook_release_rx
                        .lock()
                        .expect("boot-failure release channel poisoned")
                        .recv_timeout(Duration::from_secs(10));
                }),
                ..Default::default()
            },
        )?;
        let h = r.handle();
        let mut responses = (0..5)
            .map(|_| h.handle_blocking(req("/", "failboot_worker_tests/failboot-worker.php")))
            .collect::<Result<Vec<_>, _>>()?;
        let fifth = responses.pop().expect("five responses were queued");
        let mut statuses = Vec::with_capacity(5);
        for response in responses {
            let (s, _) = drain(response);
            statuses.push(s);
        }

        hook_entered_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("worker did not reach the unhealthy boot-failure hook");
        // A new Windows interpreter generation clears the current health in the bind-once slot. Capture generation 1 at the threshold before generation 2 starts.
        let unhealthy = r.scoreboard().unhealthy;
        let _ = hook_release_tx.send(());

        let (s, _) = drain(fifth);
        statuses.push(s);
        drop(h);
        r.shutdown();
        let _ = done_tx.send((unhealthy, statuses));
        Ok(())
    });

    let (unhealthy, statuses) = done_rx
        .recv_timeout(Duration::from_secs(15))
        .expect("boot-failing worker hung (unhealthy/Drop regression)");
    assert!(
        statuses.iter().all(|&s| s == 503),
        "each boot-failed job must 503 (got {statuses:?})"
    );
    assert_eq!(
        unhealthy, 1,
        "5 consecutive boot failures must flag the worker unhealthy"
    );
    scenario.join().expect("scenario thread panicked")?;
    Ok(())
}

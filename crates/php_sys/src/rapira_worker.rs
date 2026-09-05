use tracing::error;

use crate::{
    callbacks::*, diagnostics::error_type_to_level, scoreboard::sb_update, start::pull_job,
    types::Outcome,
};
use std::{
    borrow::Cow,
    cell::RefCell,
    os::raw::c_int,
    path::{Path, PathBuf},
};

use crate::{
    callbacks::guard,
    context::{bind_server_context, ctx, populate_request_context, unbind_server_context},
    executor::run_script,
    php_request_startup, rapira_eg, rapira_pg, rapira_run_handler,
    types::Context,
    zend_fcall_info, zend_fcall_info_cache, *,
};

thread_local! {
    static WORKER: RefCell<Option<WorkerChan>> = const { RefCell::new(None) };
}

const UNHEALTHY_AFTER: u32 = 5;

enum Cycle {
    Stop,
    Recycle,
    Failed,
    Restart,
}

pub enum WorkerExit {
    Closed,
    Restart,
}

// Keep these values synchronized with the RAPIRA_HANDLE_* values in wrapper.h.
#[repr(i32)]
enum HandleAction {
    Stop = 0,
    Continue = 1,
    Recycle = 2,
}

struct WorkerChan {
    first_call: bool,
    recycle: bool,
}

fn worker_recycle() -> bool {
    WORKER.with_borrow(|w| w.as_ref().is_some_and(|wc| wc.recycle))
}

fn set_worker_recycle() {
    WORKER.with_borrow_mut(|w| {
        if let Some(wc) = w.as_mut() {
            wc.recycle = true;
        }
    });
}

/// Logs PG(last_error_message) before php_request_shutdown releases it (main/main.c:2024).
fn run_cycle(script: &Path) -> Cycle {
    crate::exchange::cycle_reset();
    let started = unsafe { php_request_startup() } == SUCCESS;
    if started {
        unsafe { run_script(script) };
    } else {
        error!(target: "rapira", "php_request_startup() failed");
    }

    let recycle = WORKER.with_borrow_mut(|w| {
        w.as_mut().is_some_and(|wc| {
            wc.first_call = true;
            std::mem::take(&mut wc.recycle)
        })
    });

    if crate::exchange::served_any() {
        sb_update(scoreboard::Event::Healthy);
    }

    log_and_clear_last_error();
    if Outcome::from_c(unsafe { rapira_request_shutdown() }) == Outcome::Bailout {
        error!(target: "rapira", "php_request_shutdown() bailed; restarting the PHP thread");
        sb_update(scoreboard::Event::Restart);
        return Cycle::Restart;
    }

    if crate::exchange::closed_seen() {
        Cycle::Stop
    } else if recycle || crate::exchange::served_any() || crate::exchange::received_any() {
        Cycle::Recycle
    } else {
        Cycle::Failed
    }
}

pub fn rapira_worker(script: PathBuf) -> WorkerExit {
    WORKER.with_borrow_mut(|w| {
        *w = Some(WorkerChan {
            first_call: true,
            recycle: false,
        })
    });

    let mut failures: u32 = 0;
    let exit = loop {
        match run_cycle(&script) {
            Cycle::Stop => break WorkerExit::Closed,
            Cycle::Restart => break WorkerExit::Restart,
            Cycle::Recycle => {
                sb_update(scoreboard::Event::Recycled);
                failures = 0;
            }
            Cycle::Failed => {
                failures += 1;
                if failures == UNHEALTHY_AFTER {
                    error!(target: "rapira", "worker keeps failing to boot; flagged unhealthy");
                    sb_update(scoreboard::Event::Unhealthy);
                }
                match pull_job() {
                    None => break WorkerExit::Closed,
                    Some(mut job) => {
                        send_error_head(&mut job, 503);
                        job.finish(false);
                        sb_update(scoreboard::Event::Shed);
                    }
                }
            }
        }
    };
    crate::exchange::reclaim_current();
    log_and_clear_last_error();
    exit
}

/// # Safety
/// The C handle_request function calls this on the resident worker thread during an active request. `fci` and `fcc` must be valid. The unconditional unbind supports the caught panic path, where SG(server_context) would otherwise point to a released job.
#[unsafe(no_mangle)]
pub extern "C" fn rapira_rs_handle_request(
    fci: *mut zend_fcall_info,
    fcc: *mut zend_fcall_info_cache,
) -> c_int {
    let action = guard(HandleAction::Recycle, || handle_request_impl(fci, fcc));
    unbind_server_context();
    action as c_int
}

/// Flushes before rapira_request_teardown. SG(sapi_headers) contains the response head, including the status, cookies, and the 500 response from php_error_cb. Teardown destroys this value.
fn handle_request_impl(fci: *mut zend_fcall_info, fcc: *mut zend_fcall_info_cache) -> HandleAction {
    let Some(mut job) = next_job() else {
        return if worker_recycle() {
            HandleAction::Recycle
        } else {
            HandleAction::Stop
        };
    };

    bind_server_context(&mut job);
    unsafe {
        populate_request_context(&mut job);
        rapira_release_temporary_streams();
    }

    let mut outcome = Outcome::from_c(unsafe { rapira_request_activate() });
    if outcome != Outcome::Bailout {
        unsafe {
            crate::context::apply_proto_num(&job);
            outcome = Outcome::from_c(rapira_run_handler(fci, fcc));
        }
    }

    job.tearing_down = true;
    let flushed = match outcome {
        Outcome::Bailout | Outcome::Throw => Outcome::from_c(unsafe { rapira_finish_output() }),
        _ => Outcome::Ok,
    };
    let teardown: Outcome = Outcome::from_c(unsafe { rapira_request_teardown() });

    let recycle: bool = [outcome, flushed, teardown].contains(&Outcome::Bailout);
    let errored: bool = recycle || outcome == Outcome::Throw;
    let truncated: bool = finalize_response(&mut job, errored);

    log_and_clear_last_error();
    unbind_server_context();
    sb_update(scoreboard::Event::Handled(errored));
    if recycle {
        set_worker_recycle();
    }
    job.finish(truncated);
    crate::exchange::note_served();
    if recycle {
        HandleAction::Recycle
    } else {
        HandleAction::Continue
    }
}

/// The first call ends the startup request that php_request_startup() created before the worker processes a job.
fn next_job() -> Option<Context> {
    WORKER.with_borrow_mut(|w| {
        let wc = w.as_mut()?;
        if std::mem::take(&mut wc.first_call) {
            let outcome = Outcome::from_c(unsafe { rapira_request_teardown() });
            if outcome == Outcome::Bailout {
                error!(target: "rapira", "rapira_request_teardown() bailed on first call; recycling");
                wc.recycle = true;
                return None;
            }
            // Exclude startup registrations from per-job shutdown. They run when the cycle ends.
            unsafe { rapira_stash_boot_shutdown_functions() };
            sb_update(scoreboard::Event::Healthy);
        }
        log_and_clear_last_error();
        loop {
            match pull_job() {
                Some(job) => {
                    if job.sender.as_ref().is_some_and(|s| s.is_closed()) {
                        sb_update(scoreboard::Event::Handled(true));
                        continue;
                    }
                    crate::exchange::note_received();
                    return Some(job);
                }
                None => {
                    crate::exchange::note_closed();
                    return None;
                }
            }
        }
    })
}

/// # Safety
/// rapira_finish_request calls this function from C on a worker thread during an active request. `SG(server_context)` must contain the request `Context`.
#[unsafe(no_mangle)]
pub extern "C" fn rapira_rs_finish_response() {
    guard((), || unsafe {
        if let Some(c) = ctx() {
            c.finish(false);
        }
    });
}

/// clear_last_error() does not clear last_error_type or lineno (main/main.c:1307-1316), so only the message pointer indicates an error. php_error_cb applies EG(error_reporting) only after it sets this pointer (main/main.c:1394-1411).
fn log_and_clear_last_error() {
    unsafe {
        let pg = rapira_pg();
        let msg = (*pg).last_error_message;
        if !msg.is_null() {
            let (level, label) =
                error_type_to_level((*pg).last_error_type, (*rapira_eg()).error_reporting);
            crate::diagnostics::php_log!(
                level,
                "{label}: {} in {}:{}",
                zstr_lossy(&*msg),
                zstr_lossy(&*(*pg).last_error_file),
                (*pg).last_error_lineno
            );
        }
        rapira_clear_last_error();
    }
}

fn zstr_lossy(s: &zend_string) -> Cow<'_, str> {
    let bytes = unsafe { std::slice::from_raw_parts(s.val.as_ptr().cast::<u8>(), s.len) };
    String::from_utf8_lossy(bytes)
}

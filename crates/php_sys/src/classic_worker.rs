use std::{fs::File, io::ErrorKind};

use crate::{
    callbacks::{finalize_response, send_error_head},
    context::{bind_server_context, populate_request_context, unbind_server_context},
    executor::run_script,
    scoreboard::{Event, sb_update},
    start::pull_job,
    types::Context,
    *,
};

fn status_for_open_error(kind: ErrorKind) -> u16 {
    match kind {
        ErrorKind::NotFound => 404,
        ErrorKind::PermissionDenied => 403,
        _ => 500,
    }
}

pub(crate) fn classic_worker() {
    while let Some(mut job) = pull_job() {
        let (event, truncated) = classic_executor(&mut job);
        sb_update(event);
        job.finish(truncated);
    }
}

// run_script is also false after exit() or die(). Only unclean_shutdown or a new last_error_message indicates a failure.
fn classic_executor(job: &mut Context) -> (Event, bool) {
    bind_server_context(job);
    let (is_errored, truncated) = unsafe {
        populate_request_context(job);
        if php_request_startup() == FAILURE {
            send_error_head(job, 500);
            rapira_request_shutdown();
            unbind_server_context();
            return (Event::Handled(true), false);
        }
        crate::context::apply_proto_num(job);

        let exec_err: bool = match File::open(&job.req.script_filename) {
            Err(e) => {
                send_error_head(job, status_for_open_error(e.kind()));
                true
            }
            Ok(_) => {
                let failed = !run_script(&job.req.script_filename);
                let pg = rapira_pg();
                failed
                    && ((*rapira_cg()).unclean_shutdown
                        || (!(*pg).last_error_message.is_null()
                            && (*pg).last_error_type & E_FATAL_ERRORS as i32 != 0))
            }
        };
        job.tearing_down = true;
        rapira_request_shutdown();

        let truncated = finalize_response(job, exec_err);

        (exec_err, truncated)
    };

    unbind_server_context();
    (Event::Handled(is_errored), truncated)
}

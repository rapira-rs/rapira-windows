use crate::{
    callbacks::{fatal_error_recorded, finalize_response, send_error_head},
    context::{bind_server_context, populate_request_context, unbind_server_context},
    scoreboard::{Event, sb_update},
    start::{pull_unit, run_script},
    types::Context,
    *,
};

pub(crate) fn classic_worker() {
    while let Some(unit) = pull_unit() {
        if unit.cancelled() {
            sb_update(Event::Shed);
            continue;
        }
        let mut ctx = unit.into_cgi();
        let Executed { event, truncated } = classic_executor(&mut ctx);
        sb_update(event);
        ctx.finish(truncated);
    }
}

struct Executed {
    event: Event,
    truncated: bool,
}

// A recorded fatal error distinguishes failure from a normal exit.
fn classic_executor(ctx: &mut Context) -> Executed {
    bind_server_context(ctx);
    let (is_errored, truncated) = unsafe {
        populate_request_context(ctx);
        if php_request_startup() == FAILURE {
            send_error_head(ctx, 500);
            rapira_request_shutdown();
            unbind_server_context();
            return Executed {
                event: Event::Handled(true),
                truncated: false,
            };
        }
        crate::context::apply_proto_num(ctx);

        let failed = crate::context::with_script(|script| {
            !run_script(std::path::Path::new(&script.filename))
        });
        let exec_err = failed && fatal_error_recorded();
        ctx.tearing_down = true;
        ctx.errored = exec_err;
        rapira_request_shutdown();

        let truncated = finalize_response(ctx, exec_err);

        (exec_err, truncated)
    };

    unbind_server_context();
    Executed {
        event: Event::Handled(is_errored),
        truncated,
    }
}

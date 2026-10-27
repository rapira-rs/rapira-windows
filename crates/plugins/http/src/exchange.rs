use tokio::sync::mpsc::Sender;

use rapira_sapi::callbacks::send_error_head;
use rapira_sapi::types::{Context, Frame, ReqC, Request};
use rapira_sapi::work::{Held, Work};
use rapira_sapi::zend_object;

use crate::php::{ExchangeState, exchange_from};

// Keep up to 1 MiB of 64 KiB writes queued before the PHP thread waits.
pub(crate) const FRAME_CAP: usize = 16;

/// One HTTP request on the intake.
pub(crate) struct Exchange {
    req: Request,
    sender: Sender<Frame>,
    cgi: Option<ReqC>,
}

impl Exchange {
    /// Build the CGI view on the transport thread for classic and worker modes.
    pub(crate) fn new(req: Request, sender: Sender<Frame>, script: Option<&str>) -> Self {
        let cgi = script.map(|script| ReqC::build(&req, script));
        Self { req, sender, cgi }
    }
}

impl Work for Exchange {
    fn cancelled(&self) -> bool {
        self.sender.is_closed()
    }

    unsafe fn attach(self: Box<Self>, obj: *mut zend_object) -> *mut dyn Held {
        let Self { req, sender, .. } = *self;
        let ptr = Box::into_raw(Box::new(ExchangeState::new(req, sender)));
        // SAFETY: the caller passes a live Rapira\Internal\Http\Exchange object.
        unsafe { (*exchange_from(obj)).unit = ptr.cast() };
        ptr
    }

    fn into_cgi(self: Box<Self>) -> Context {
        let cgi = self.cgi.unwrap_or_else(|| ReqC::build(&self.req, ""));
        Context::new(self.req, self.sender, cgi)
    }

    fn shed(self: Box<Self>) {
        let mut ctx = self.into_cgi();
        send_error_head(&mut ctx, 503);
        ctx.finish(false);
    }
}

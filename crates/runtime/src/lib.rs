use extension_api::{Extension, Php, PrepareCtx};
use http::header::CONTENT_TYPE;
use php_sys::RapiraHandle;
use std::future::Future;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::Runtime;
use tokio::sync::watch;
use tokio::task::JoinSet;

pub mod multipart;

type Outcome = std::result::Result<(), String>;
type BoxFuture = Pin<Box<dyn Future<Output = Outcome> + Send>>;

/// Object-safe interface that prepares each extension before PHP starts and then starts it on the runtime.
trait ErasedExt: Send {
    fn prepare(&mut self, ctx: &mut PrepareCtx) -> anyhow::Result<()>;
    fn launch(self: Box<Self>, php: Php, stop: watch::Receiver<bool>, grace: Duration)
    -> BoxFuture;
}

impl<E: Extension> ErasedExt for E {
    fn prepare(&mut self, ctx: &mut PrepareCtx) -> anyhow::Result<()> {
        Extension::prepare(self, ctx)
    }

    fn launch(
        self: Box<Self>,
        php: Php,
        stop: watch::Receiver<bool>,
        grace: Duration,
    ) -> BoxFuture {
        Box::pin(drive(*self, php, stop, grace))
    }
}

struct Registered {
    name: String,
    ext: Box<dyn ErasedExt>,
}

#[derive(Default)]
pub struct ExtensionRuntime {
    exts: Vec<Registered>,
}

impl ExtensionRuntime {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register<E: Extension>(&mut self, config: E::Config) {
        let ext = E::init(config);
        self.exts.push(Registered {
            name: ext.name().to_string(),
            ext: Box::new(ext),
        });
    }

    /// Runs every extension's `prepare` in registration order before PHP boots.
    pub fn prepare_all(&mut self, ctx: &mut PrepareCtx) -> anyhow::Result<()> {
        use anyhow::Context;
        for Registered { name, ext } in &mut self.exts {
            ext.prepare(ctx)
                .with_context(|| format!("extension {name}: prepare failed"))?;
        }
        Ok(())
    }

    pub fn run(self, rapira: RapiraHandle, script: PathBuf) -> Running {
        self.run_with_options(rapira, script, RuntimeOptions::default())
    }

    /// One worker thread runs the `drive` future of each extension.
    pub fn run_with_options(
        self,
        rapira: RapiraHandle,
        script: PathBuf,
        opts: RuntimeOptions,
    ) -> Running {
        let grace = opts.grace;
        let php = Php::new(Arc::new(RapiraBackend::new(rapira, &script, opts)));
        let (stop_tx, stop_rx) = watch::channel(false);
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_time()
            .thread_name("rapira-ext")
            .build()
            .expect("build extension runtime");

        let mut tasks: JoinSet<Result<(), String>> = JoinSet::new();
        for Registered { name, ext } in self.exts {
            let (php, stop) = (php.clone(), stop_rx.clone());
            let fut = ext.launch(php, stop, grace);
            tasks.spawn_on(
                async move {
                    let outcome = fut.await;
                    match &outcome {
                        Ok(()) => tracing::info!(target: "ext", "{name} finished"),
                        Err(msg) => tracing::error!(target: "ext", "{name}: {msg}"),
                    }
                    outcome
                },
                rt.handle(),
            );
        }

        Running { rt, tasks, stop_tx }
    }
}

pub struct RuntimeOptions {
    pub grace: Duration,
    /// Multipart limits for host parsing. Only a dispatcher handle reads them. Worker mode uses the rfc1867 parser from php-src through read_post.
    pub uploads: Arc<multipart::Limits>,
}

impl Default for RuntimeOptions {
    fn default() -> Self {
        Self {
            grace: Duration::from_secs(30),
            uploads: Arc::new(multipart::Limits::default()),
        }
    }
}

struct RapiraBackend {
    rapira: RapiraHandle,
    uploads: Arc<multipart::Limits>,
}

fn map_addr(a: extension_api::Addr) -> php_sys::types::Addr {
    match a {
        extension_api::Addr::Inet(sa) => php_sys::types::Addr::Inet(sa),
        extension_api::Addr::Unix(p) => php_sys::types::Addr::Unix(p),
    }
}

fn map_tls(t: extension_api::Tls) -> php_sys::types::TlsView {
    php_sys::types::TlsView {
        version: t.version,
        cipher: t.cipher,
        alpn: t.alpn,
        server_name: t.server_name,
        cert: t.cert.map(|c| php_sys::types::ClientCertView {
            serial: c.serial,
            organization: c.organization,
            fingerprint: c.fingerprint,
        }),
    }
}

fn parse_err(e: multipart::ParseError) -> anyhow::Error {
    match e {
        multipart::ParseError::Rejected(r) => anyhow::Error::new(r),
        multipart::ParseError::Io(io) => anyhow::Error::new(io).context("upload spool failed"),
    }
}

impl RapiraBackend {
    fn new(rapira: RapiraHandle, filename: &Path, opts: RuntimeOptions) -> Self {
        php_sys::set_script(filename);
        Self {
            rapira,
            uploads: opts.uploads,
        }
    }

    /// Parses multipart data before enqueueing the request. A rejected body does not increment the pending or active counters.
    async fn to_request(
        &self,
        mut req: extension_api::Request,
    ) -> anyhow::Result<php_sys::Request> {
        let content_type = req.headers.get(CONTENT_TYPE).map(|v| v.as_bytes().to_vec());
        let content_length = req.body.len() as i64;

        // Content-Type is a singleton field under RFC 9110 section 8.3. Repeated lines can cause the host and PHP to use different boundaries.
        // https://www.rfc-editor.org/rfc/rfc9110#section-8.3
        if self.rapira.dispatcher() && !req.body.is_empty() {
            let lines = req.headers.get_all(CONTENT_TYPE);
            if lines.iter().nth(1).is_some()
                && lines.iter().any(|v| multipart::is_multipart(v.as_bytes()))
            {
                return Err(anyhow::Error::new(extension_api::Rejected {
                    status: 400,
                    reason: "repeated content-type field lines with a multipart body".into(),
                }));
            }
        }

        let body = if self.rapira.dispatcher()
            && !req.body.is_empty()
            && let Some(ct) = content_type.as_deref()
            && multipart::is_multipart(ct)
        {
            let boundary = multipart::boundary(ct).map_err(parse_err)?;
            let bytes = std::mem::take(&mut req.body);
            let limits = Arc::clone(&self.uploads);
            let parsed =
                tokio::task::spawn_blocking(move || multipart::parse(&bytes, &boundary, &limits))
                    .await
                    .map_err(|e| anyhow::anyhow!("multipart parse task failed: {e}"))?;
            php_sys::types::Body::Multipart(parsed.map_err(parse_err)?)
        } else {
            php_sys::types::Body::Raw(Cursor::new(std::mem::take(&mut req.body)))
        };

        Ok(php_sys::Request {
            method: req.method,
            https: req.https,
            protocol: req.protocol,
            target: req.target,
            authority: req.authority,
            remote: map_addr(req.remote),
            server: map_addr(req.server),
            server_name: req.server_name,
            server_port: req.server_port,
            content_type,
            content_length,
            body,
            headers: req.headers,
            uri: req.uri,
            received_at: req.received_at,
            tls: req.tls.map(map_tls),
        })
    }
}

impl extension_api::Backend for RapiraBackend {
    /// `Reply` directly contains the frame receiver, so dropping it signals the exchange layer that the client disconnected.
    fn exec(
        &self,
        req: extension_api::Request,
    ) -> Pin<Box<dyn Future<Output = extension_api::Result<extension_api::Reply>> + Send + '_>>
    {
        Box::pin(async move {
            let req = self.to_request(req).await?;
            let rx = self.rapira.handle(req).await.map_err(|e| {
                anyhow::Error::new(extension_api::Rejected {
                    status: match e {
                        php_sys::HandleError::Saturated => 503,
                        php_sys::HandleError::Stopped => 500,
                    },
                    reason: e.to_string(),
                })
            })?;
            Ok(extension_api::Reply::new(Box::new(FrameSource(rx))))
        })
    }
}

struct FrameSource(tokio::sync::mpsc::Receiver<php_sys::Frame>);

impl extension_api::ReplySource for FrameSource {
    fn poll_next(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<extension_api::ReplyEvent>> {
        self.0.poll_recv(cx).map(|opt| {
            opt.map(|frame| match frame {
                php_sys::Frame::Interim(h) => extension_api::ReplyEvent::Interim {
                    status: h.status,
                    headers: h.headers,
                },
                php_sys::Frame::Head {
                    head,
                    content_length,
                    bodiless,
                } => extension_api::ReplyEvent::Head {
                    status: head.status,
                    headers: head.headers,
                    content_length,
                    bodiless,
                },
                php_sys::Frame::Chunk(b) => extension_api::ReplyEvent::Chunk(b),
                php_sys::Frame::File { file, offset, len } => {
                    extension_api::ReplyEvent::File { file, offset, len }
                }
                php_sys::Frame::End {
                    trailers,
                    truncated,
                } => extension_api::ReplyEvent::End {
                    trailers,
                    truncated,
                },
            })
        })
    }
}

/// During stop, the function drops the `run` future first. This releases `&mut ext` so `shutdown` can drain within `grace`.
async fn drive<E: Extension>(
    mut ext: E,
    php: Php,
    mut stop: watch::Receiver<bool>,
    grace: Duration,
) -> Outcome {
    let finished = {
        let run = ext.run(php);
        tokio::pin!(run);
        tokio::select! {
            outcome = &mut run => Some(outcome),
            _ = stop.wait_for(|stopping| *stopping) => None,
        }
    };
    match finished {
        Some(outcome) => outcome.map_err(|e| format!("run failed: {e:#}")),
        None => match tokio::time::timeout(grace, ext.shutdown()).await {
            Ok(result) => result.map_err(|e| format!("shutdown failed: {e:#}")),
            Err(_) => Err("shutdown timed out".into()),
        },
    }
}

/// Keeps console control handling installed while PHP can have live threads.
pub struct ShutdownWatcher(());

impl ShutdownWatcher {
    /// Installs the console control handler before PHP starts.
    pub fn install() -> std::io::Result<Self> {
        win_ctrl::install()?;
        Ok(Self(()))
    }

    fn register(&self, stop_tx: watch::Sender<bool>) {
        win_ctrl::register(stop_tx);
    }
}

impl Drop for ShutdownWatcher {
    fn drop(&mut self) {
        win_ctrl::uninstall();
    }
}

mod win_ctrl {
    use std::io;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Mutex, MutexGuard, PoisonError};

    use tokio::sync::watch;
    use windows_sys::Win32::System::Console::{
        CTRL_BREAK_EVENT, CTRL_C_EVENT, SetConsoleCtrlHandler,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
    use windows_sys::core::BOOL;

    /// One sender per running extension runtime. A control event reaches every runtime.
    static STOP_TXS: Mutex<Vec<watch::Sender<bool>>> = Mutex::new(Vec::new());
    static ASKED: AtomicBool = AtomicBool::new(false);

    fn senders(list: &Mutex<Vec<watch::Sender<bool>>>) -> MutexGuard<'_, Vec<watch::Sender<bool>>> {
        list.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Adds `stop_tx` and replays an event that arrived before the registration.
    fn register_stop(
        asked: &AtomicBool,
        list: &Mutex<Vec<watch::Sender<bool>>>,
        stop_tx: watch::Sender<bool>,
    ) {
        let mut list = senders(list);
        if asked.load(Ordering::SeqCst) {
            let _ = stop_tx.send(true);
        }
        list.push(stop_tx);
    }

    // Windows calls this function on a new thread, so watch and logger locks are valid.
    // CLOSE, LOGOFF and SHUTDOWN retain the default handler.
    // https://learn.microsoft.com/en-us/windows/console/handlerroutine
    unsafe extern "system" fn handler(ctrl_type: u32) -> BOOL {
        match ctrl_type {
            CTRL_C_EVENT | CTRL_BREAK_EVENT => {
                if ASKED.swap(true, Ordering::SeqCst) {
                    // SAFETY: This terminates the process without DLL detach and does not return.
                    // https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-terminateprocess
                    // https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-exitprocess
                    unsafe { TerminateProcess(GetCurrentProcess(), 130) };
                }
                tracing::info!(target: "rapira", "shutdown event received; draining extensions");
                for tx in senders(&STOP_TXS).iter() {
                    let _ = tx.send(true);
                }
                1
            }
            _ => 0,
        }
    }

    pub(super) fn install() -> io::Result<()> {
        ASKED.store(false, Ordering::SeqCst);
        senders(&STOP_TXS).clear();
        // SAFETY: handler has the required ABI and remains valid for the process lifetime.
        // https://learn.microsoft.com/en-us/windows/console/setconsolectrlhandler
        if unsafe { SetConsoleCtrlHandler(Some(handler), 1) } == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    pub(super) fn register(stop_tx: watch::Sender<bool>) {
        register_stop(&ASKED, &STOP_TXS, stop_tx);
    }

    pub(super) fn uninstall() {
        // SAFETY: removes the handler installed above.
        unsafe { SetConsoleCtrlHandler(Some(handler), 0) };
        senders(&STOP_TXS).clear();
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// Both runtimes see the event: one registered before it, one after it.
        #[test]
        fn registration_replays_an_early_request_to_every_runtime() {
            let asked = AtomicBool::new(false);
            let list = Mutex::new(Vec::new());
            let (first_tx, first_rx) = watch::channel(false);
            register_stop(&asked, &list, first_tx);
            assert!(!*first_rx.borrow());

            assert!(!asked.swap(true, Ordering::SeqCst));
            for tx in senders(&list).iter() {
                let _ = tx.send(true);
            }
            let (second_tx, second_rx) = watch::channel(false);
            register_stop(&asked, &list, second_tx);

            assert!(*first_rx.borrow());
            assert!(*second_rx.borrow());
            assert_eq!(senders(&list).len(), 2);
        }
    }
}

/// Stop handle that standard threads can call because `watch::Sender::send` does not require a runtime.
#[derive(Clone)]
pub struct Stopper(watch::Sender<bool>);

impl Stopper {
    pub fn stop(&self) {
        let _ = self.0.send(true);
    }
}

pub struct Running {
    rt: Runtime,
    tasks: JoinSet<Outcome>,
    stop_tx: watch::Sender<bool>,
}

impl Running {
    pub fn join(mut self) -> Vec<Outcome> {
        self.drain_all()
    }

    pub fn stop(self) -> Vec<Outcome> {
        let _ = self.stop_tx.send(true);
        self.join()
    }

    pub fn stopper(&self) -> Stopper {
        Stopper(self.stop_tx.clone())
    }

    /// The first Ctrl+C or Ctrl+Break event drains extensions. A second event forces exit 130. Keep `watcher` until PHP shuts down.
    pub fn serve(mut self, watcher: &ShutdownWatcher) -> Vec<Outcome> {
        watcher.register(self.stop_tx.clone());
        self.drain_all()
    }

    fn drain_all(&mut self) -> Vec<Outcome> {
        self.rt.block_on(async {
            let mut out = Vec::with_capacity(self.tasks.len());
            while let Some(joined) = self.tasks.join_next().await {
                out.push(joined.unwrap_or_else(|_| Err("driver task panicked".into())));
            }
            out
        })
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.stop_tx.send(true);
        let _ = self.drain_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `parse_err` preserves both error types. The error chain contains `io::Error`, and `Rejected` remains downcastable.
    #[test]
    fn parse_err_keeps_the_typed_causes() {
        let io = parse_err(multipart::ParseError::Io(std::io::Error::other(
            "disk full",
        )));
        assert!(io.chain().any(|c| c.is::<std::io::Error>()));
        assert!(io.downcast_ref::<extension_api::Rejected>().is_none());

        let rejected = parse_err(multipart::ParseError::Rejected(extension_api::Rejected {
            status: 413,
            reason: "too big".into(),
        }));
        assert_eq!(
            rejected
                .downcast_ref::<extension_api::Rejected>()
                .map(|r| r.status),
            Some(413)
        );
    }
}

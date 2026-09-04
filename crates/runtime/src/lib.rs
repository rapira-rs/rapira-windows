use extension_api::{Extension, Php, PrepareCtx};
use php_sys::RapiraHandle;
use std::future::Future;
use std::io::Cursor;
use std::path::PathBuf;
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

    pub fn register<E: Extension>(&mut self, config: E::Config) -> anyhow::Result<()> {
        let ext = E::init(config);
        let name = ext.name().to_string();
        if self.exts.iter().any(|e| e.name == name) {
            anyhow::bail!("duplicate extension {name:?}");
        }
        self.exts.push(Registered {
            name,
            ext: Box::new(ext),
        });
        Ok(())
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

    /// One runtime thread runs the extension tasks and their shutdown timeouts.
    pub fn run_with_options(
        self,
        rapira: RapiraHandle,
        script: PathBuf,
        opts: RuntimeOptions,
    ) -> Running {
        let grace = opts.grace;
        let php = Php::new(Arc::new(RapiraBackend::new(rapira, script, opts)));
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
    filename: PathBuf,
    document_root: String,
    script_name: String,
    dispatcher: bool,
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
    fn new(rapira: RapiraHandle, filename: PathBuf, opts: RuntimeOptions) -> Self {
        let document_root = filename
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let script_name = filename
            .file_name()
            .map_or_else(|| "/".to_string(), |f| format!("/{}", f.to_string_lossy()));
        let dispatcher = rapira.dispatcher();
        Self {
            rapira,
            filename,
            document_root,
            script_name,
            dispatcher,
            uploads: opts.uploads,
        }
    }

    /// Parses multipart data before enqueueing the request. A rejected body does not increment the pending or active counters.
    async fn to_request(
        &self,
        mut req: extension_api::Request,
    ) -> anyhow::Result<php_sys::Request> {
        let query = req.uri.split_once('?').map_or("", |(_, q)| q).to_string();
        let content_type = req
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
            .map(|(_, v)| v.clone());
        let content_length = req.body.len() as i64;

        // Content-Type is a singleton field under RFC 9110 section 8.3. Repeated lines can cause the host and PHP to use different boundaries.
        // https://www.rfc-editor.org/rfc/rfc9110#section-8.3
        if self.dispatcher && !req.body.is_empty() {
            let mut ct_lines = 0usize;
            let mut any_multipart = false;
            for (k, v) in &req.headers {
                if k.eq_ignore_ascii_case("content-type") {
                    ct_lines += 1;
                    any_multipart = any_multipart || multipart::is_multipart(v);
                }
            }
            if ct_lines > 1 && any_multipart {
                return Err(anyhow::Error::new(extension_api::Rejected {
                    status: 400,
                    reason: "repeated content-type field lines with a multipart body".into(),
                }));
            }
        }

        let body = if self.dispatcher
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
            php_sys::types::Body::Raw(Box::new(Cursor::new(std::mem::take(&mut req.body))))
        };

        Ok(php_sys::Request {
            method: req.method,
            https: req.https,
            query,
            protocol: req.protocol,
            target: req.target.filter(|t| !t.is_empty()),
            authority: req.authority.filter(|a| !a.is_empty()),
            remote: map_addr(req.remote),
            server: map_addr(req.server),
            server_name: req.server_name,
            server_port: req.server_port,
            script_name: self.script_name.clone(),
            document_root: self.document_root.clone(),
            script_filename: self.filename.clone(),
            content_type,
            content_length,
            body,
            headers: req.headers,
            server_vars: Vec::new(),
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
                    body_coded,
                } => extension_api::ReplyEvent::Head {
                    status: head.status,
                    headers: head.headers,
                    content_length,
                    bodiless,
                    body_coded,
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
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicBool, Ordering};

    use tokio::sync::watch;
    use windows_sys::Win32::System::Console::{
        CTRL_BREAK_EVENT, CTRL_C_EVENT, SetConsoleCtrlHandler,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
    use windows_sys::core::BOOL;

    static STOP_TX: OnceLock<watch::Sender<bool>> = OnceLock::new();
    static ASKED: AtomicBool = AtomicBool::new(false);

    fn record_request(asked: &AtomicBool) -> bool {
        asked.swap(true, Ordering::SeqCst)
    }

    fn register_stop(
        asked: &AtomicBool,
        slot: &OnceLock<watch::Sender<bool>>,
        stop_tx: watch::Sender<bool>,
    ) {
        let _ = slot.set(stop_tx);
        if asked.load(Ordering::SeqCst)
            && let Some(tx) = slot.get()
        {
            let _ = tx.send(true);
        }
    }

    // Windows calls this function on a new thread, so watch and logger locks are valid.
    // CLOSE, LOGOFF and SHUTDOWN retain the default handler.
    // https://learn.microsoft.com/en-us/windows/console/handlerroutine
    unsafe extern "system" fn handler(ctrl_type: u32) -> BOOL {
        match ctrl_type {
            CTRL_C_EVENT | CTRL_BREAK_EVENT => {
                if record_request(&ASKED) {
                    // SAFETY: This terminates the process without DLL detach and does not return.
                    // https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-terminateprocess
                    // https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-exitprocess
                    unsafe { TerminateProcess(GetCurrentProcess(), 130) };
                }
                tracing::info!(target: "rapira", "shutdown event received; draining extensions");
                if let Some(tx) = STOP_TX.get() {
                    let _ = tx.send(true);
                }
                1
            }
            _ => 0,
        }
    }

    pub(super) fn install() -> io::Result<()> {
        ASKED.store(false, Ordering::SeqCst);
        // SAFETY: handler has the required ABI and remains valid for the process lifetime.
        // https://learn.microsoft.com/en-us/windows/console/setconsolectrlhandler
        if unsafe { SetConsoleCtrlHandler(Some(handler), 1) } == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    pub(super) fn register(stop_tx: watch::Sender<bool>) {
        register_stop(&ASKED, &STOP_TX, stop_tx);
    }

    pub(super) fn uninstall() {
        // SAFETY: removes the handler installed above.
        unsafe { SetConsoleCtrlHandler(Some(handler), 0) };
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn registration_replays_an_early_request() {
            let asked = AtomicBool::new(false);
            let slot = OnceLock::new();
            assert!(!record_request(&asked));

            let (stop_tx, stop_rx) = watch::channel(false);
            register_stop(&asked, &slot, stop_tx);

            assert!(*stop_rx.borrow());
            assert!(record_request(&asked));
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
        let mut tasks = std::mem::take(&mut self.tasks);
        self.rt.block_on(drain(&mut tasks))
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.stop_tx.send(true);
        let _ = self.drain_all();
    }
}

async fn drain(tasks: &mut JoinSet<Outcome>) -> Vec<Outcome> {
    let mut out = Vec::with_capacity(tasks.len());
    while let Some(joined) = tasks.join_next().await {
        out.push(joined.unwrap_or_else(|_| Err("driver task panicked".into())));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Prepared launchers must implement `Send` because spawned tasks receive them.
    #[test]
    fn rapira_runtime_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<ExtensionRuntime>();
    }

    use extension_api::{Reply, ReplyEvent, ReplySource};

    struct VecSource(std::collections::VecDeque<ReplyEvent>);

    impl ReplySource for VecSource {
        fn poll_next(
            &mut self,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Option<ReplyEvent>> {
            std::task::Poll::Ready(self.0.pop_front())
        }
    }

    fn reply(events: Vec<ReplyEvent>) -> Reply {
        Reply::new(Box::new(VecSource(events.into())))
    }

    fn head() -> ReplyEvent {
        ReplyEvent::Head {
            status: 200,
            headers: vec![("x-a".into(), b"1".to_vec())],
            content_length: None,
            bodiless: false,
            body_coded: false,
        }
    }

    fn end(truncated: bool) -> ReplyEvent {
        ReplyEvent::End {
            trailers: Vec::new(),
            truncated,
        }
    }

    /// Maps the four stream results to the three documented errors and `Ok`.
    #[tokio::test]
    async fn collect_maps_stream_outcomes() {
        let died = reply(Vec::new()).collect().await.unwrap_err();
        assert!(died.to_string().contains("died mid-response"), "{died:#}");

        let cut = reply(vec![head()]).collect().await.unwrap_err();
        assert!(cut.to_string().contains("truncated"), "{cut:#}");

        let cut = reply(vec![head(), end(true)]).collect().await.unwrap_err();
        assert!(cut.to_string().contains("truncated"), "{cut:#}");

        let headless = reply(vec![end(false)]).collect().await.unwrap_err();
        assert!(
            headless.to_string().contains("no response head"),
            "{headless:#}"
        );
    }

    /// Concatenates chunks in order and discards interim heads.
    #[tokio::test]
    async fn collect_concatenates_the_stream() {
        let r = reply(vec![
            ReplyEvent::Interim {
                status: 103,
                headers: Vec::new(),
            },
            head(),
            ReplyEvent::Chunk(bytes::Bytes::from_static(b"one,")),
            ReplyEvent::Chunk(bytes::Bytes::from_static(b"two")),
            end(false),
        ])
        .collect()
        .await
        .unwrap();
        assert_eq!(r.status, 200);
        assert_eq!(r.headers, vec![("x-a".to_string(), b"1".to_vec())]);
        assert_eq!(r.body, b"one,two");
    }

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

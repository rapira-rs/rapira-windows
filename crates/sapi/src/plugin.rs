use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::anyhow;
use tokio::sync::watch;

use crate::work::{DispatcherClasses, Sink};

pub use rapira_config::Mode;

/// One plugin's PHP surface.
#[derive(Clone, Copy)]
pub struct PhpPart {
    /// Registers the plugin's classes. Runs in MINIT after the base classes.
    pub register: unsafe extern "C" fn(),
    pub dispatcher: DispatcherClasses,
}

/// What the worker hands to [`Plugin::serve`].
pub struct Worker {
    pub handle: tokio::runtime::Handle,
    pub sink: Sink,
    /// Set to true once. The plugin stops accepting, drains within `drain_grace`, and returns from `serve`.
    pub stop: watch::Receiver<bool>,
    /// The plugin drain limit. The root leaves time for interpreter teardown within its shutdown deadline.
    pub drain_grace: Duration,
}

pub trait Plugin: Send + 'static {
    /// The TOML section and the dispatcher name PHP sees.
    fn name(&self) -> &'static str;
    /// The pool modes this plugin serves. The root refuses another mode at boot.
    fn modes(&self) -> &'static [Mode];
    /// The plugin's PHP surface.
    fn php(&self) -> PhpPart;
    /// Boot thread, before PHP startup, no runtime.
    fn prepare(&mut self, ctx: &mut rapira_net::PrepareCtx) -> anyhow::Result<()>;
    /// Worker side, on the plugin thread. Returns after the stop signal and the drain.
    fn serve(self: Box<Self>, worker: Worker) -> anyhow::Result<()>;
}

/// The plugin thread that [`run_plugin`] started.
pub struct Running {
    name: &'static str,
    thread: Option<JoinHandle<anyhow::Result<()>>>,
    stopper: Stopper,
}

impl Running {
    pub fn is_finished(&self) -> bool {
        self.thread.as_ref().is_none_or(JoinHandle::is_finished)
    }

    /// Joins the plugin thread. The root's shutdown timer bounds this wait.
    pub fn join(mut self) -> anyhow::Result<()> {
        let thread = self.thread.take().expect("join consumes Running");
        thread.join().map_err(|payload| {
            let msg = payload
                .downcast_ref::<&str>()
                .copied()
                .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
                .unwrap_or("unknown panic");
            anyhow!("{} panicked: {msg}", self.name)
        })?
    }
}

impl Drop for Running {
    /// An unjoined plugin thread gets the stop flag and ends on its own.
    fn drop(&mut self) {
        if self.thread.is_some() {
            self.stopper.stop();
        }
    }
}

/// Sets the stop flag from a plain thread: it needs no runtime. A stop before [`run_plugin`] also reaches the plugin.
#[derive(Clone, Default)]
pub struct Stopper {
    flag: watch::Sender<bool>,
}

impl Stopper {
    pub fn stop(&self) {
        self.flag.send_replace(true);
    }
}

/// Builds one two-worker tokio runtime with the IO and time drivers, spawns `rapira-{name}` and runs `serve` on it.
/// `stopper` stops the plugin. `drain_grace` goes to [`Worker::drain_grace`].
pub fn run_plugin(
    plugin: Box<dyn Plugin>,
    sink: Sink,
    stopper: Stopper,
    drain_grace: Duration,
) -> anyhow::Result<Running> {
    let name = plugin.name();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .thread_name(format!("rapira-{name}-io"))
        .build()
        .map_err(|e| anyhow!("building the {name} runtime: {e}"))?;
    let worker = Worker {
        handle: rt.handle().clone(),
        sink,
        stop: stopper.flag.subscribe(),
        drain_grace,
    };
    let thread = std::thread::Builder::new()
        .name(format!("rapira-{name}"))
        .spawn(move || {
            let result = plugin.serve(worker);
            // The runtime drops here, on a plain thread: a drop in an async context panics.
            drop(rt);
            result
        })
        .map_err(|e| anyhow!("spawning the {name} thread: {e}"))?;
    Ok(Running {
        name,
        thread: Some(thread),
        stopper,
    })
}

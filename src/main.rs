use clap::{Args, CommandFactory, Parser, Subcommand};
use extension_host::ExtensionHost;
use log::info;
use php_sys::{Mode, Rapira};
use rapira_config::{Listen, Overrides, Settings};
use rapira_pingora::{Config as HttpConfig, HttpServer, Listen as HttpListen};
use std::path::PathBuf;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

/// Minimal Windows-only, ZTS-only PHP application server (worker mode only):
/// pingora (HTTP/TCP) -> channel -> resident PHP worker threads -> response.
#[derive(Parser)]
#[command(name = "rapira", version)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Boot the server: start PHP workers, register the HTTP front, and serve requests.
    Serve(ServeArgs),
}

#[derive(Args)]
struct ServeArgs {
    /// Load settings from a rapira.toml. The flags below override values it sets.
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,

    /// PHP worker threads (ZTS). Defaults to the CPU count.
    #[arg(long)]
    threads: Option<usize>,

    /// Listen address: `host:port` or `:port` (all interfaces).
    #[arg(long, value_name = "ADDR")]
    listen: Option<Listen>,

    /// PHP entry (resident worker) script; overrides `pool.entrypoint` from the config file.
    #[arg(value_name = "SCRIPT")]
    script: Option<PathBuf>,
}

fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Some(Commands::Serve(args)) => serve(args),
        None => {
            Cli::command().print_help()?;
            println!();
            Ok(())
        }
    }
}

fn serve(args: ServeArgs) -> anyhow::Result<()> {
    // Collapse CLI flags, the config file, and defaults into one validated struct, resolving
    // the entry script to an absolute path.
    let settings: Settings = rapira_config::resolve(
        args.config.as_deref(),
        Overrides {
            listen: args.listen,
            threads: args.threads,
            entrypoint: args.script,
        },
    )?;
    init_logger(settings.log_level.as_deref());
    info!(target: "rapira", "rapira-windows v{} starting", env!("CARGO_PKG_VERSION"));

    let script: PathBuf = settings.pool.entrypoint.clone();

    // rapira_config::Listen and rapira_pingora::Listen are distinct types on purpose (the
    // extension crate stays independent of the config crate); core owns the one mapping.
    let http_cfg = HttpConfig {
        listen: match settings.http.listen {
            Listen::Tcp(addr) => HttpListen::Tcp(addr),
        },
        server_name: settings.http.server_name,
        server_port: settings.http.server_port,
        max_body_size: settings.http.max_body_size,
    };

    // Extensions are compiled in; register the HTTP front. With none registered there is
    // nothing to serve, so exit before booting PHP.
    let mut host: ExtensionHost = ExtensionHost::new();
    host.register::<HttpServer>(http_cfg)?;
    if host.is_empty() {
        return Ok(());
    }

    // Worker mode only: the entry script stays resident. host.run hands the same script to the
    // backend, which derives SCRIPT_FILENAME / DOCUMENT_ROOT / SCRIPT_NAME from it.
    let rapira = Rapira::start(Mode::Worker(script.clone()), settings.pool.threads)?;

    // Runs until the extension finishes or a console-ctrl event drains it.
    let outcomes = host.run(rapira.handle()?, script).serve();
    drop(rapira);
    for outcome in outcomes {
        outcome.map_err(|msg| anyhow::anyhow!("extension failed: {msg}"))?;
    }
    Ok(())
}

/// RUST_LOG wins; otherwise fall back to the config's `log_level`; otherwise env_logger's
/// default. Initialized once, after config is resolved.
fn init_logger(config_level: Option<&str>) {
    let mut builder = env_logger::Builder::from_env(env_logger::Env::default());
    if std::env::var_os("RUST_LOG").is_none() {
        if let Some(level) = config_level {
            builder.parse_filters(level);
        }
    }
    builder.init();
}

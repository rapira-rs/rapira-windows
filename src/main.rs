use clap::{Args, CommandFactory, Parser, Subcommand};
use rapira_config::PoolSettings;
use rapira_net::PrepareCtx;
use rapira_sapi::plugin::{Mode, Plugin};
use std::{path::PathBuf, process::ExitCode};

mod logging;
mod pidfile;
mod settings;
mod worker;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[derive(Parser)]
#[command(name = "rapira", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Prepare the plugins, start PHP, and serve requests.
    Serve(ServeArgs),
}

#[derive(Args)]
struct ServeArgs {
    /// Path to rapira.toml. Relative paths resolve against its directory.
    #[arg(value_name = "CONFIG")]
    config: PathBuf,
}

fn main() -> ExitCode {
    let result = match Cli::parse().command {
        Some(Commands::Serve(args)) => serve(args),
        None => Cli::command()
            .print_help()
            .map(|()| {
                println!();
                ExitCode::SUCCESS
            })
            .map_err(Into::into),
    };
    match result {
        Ok(code) => code,
        Err(error) => {
            eprintln!("{error:#}");
            ExitCode::FAILURE
        }
    }
}

fn check_mode(name: &str, served: &[Mode], mode: Mode) -> anyhow::Result<()> {
    anyhow::ensure!(
        served.contains(&mode),
        "{name}.pool.mode = {mode}: this plugin serves {}",
        served
            .iter()
            .map(Mode::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    );
    Ok(())
}

fn serve(args: ServeArgs) -> anyhow::Result<ExitCode> {
    use anyhow::Context;
    let settings = settings::resolve(&args.config)?;
    logging::init(&settings.log);
    tracing::info!(target: "rapira", "rapira_windows v{} starting", env!("CARGO_PKG_VERSION"));
    let _pidfile = settings
        .supervisor
        .pidfile
        .as_deref()
        .map(pidfile::PidFile::write)
        .transpose()?;
    let mut plugins: Vec<(Box<dyn Plugin>, PoolSettings)> = Vec::new();
    if let Some(http) = settings.http {
        let pool = http.pool.clone();
        plugins.push((Box::new(rapira_http::Server::from_settings(http)), pool));
    }
    if let Some(grpc) = settings.grpc {
        let pool = grpc.pool.clone();
        plugins.push((Box::new(rapira_grpc::Server::from_settings(grpc)?), pool));
    }
    let threads = plugins
        .iter()
        .map(|(_, pool)| pool.processes)
        .sum::<usize>();
    anyhow::ensure!(
        threads <= rapira_scoreboard::SB_MAX_SLOTS,
        "the total interpreter count exceeds {}",
        rapira_scoreboard::SB_MAX_SLOTS
    );
    let mut prepare = PrepareCtx::new();
    for (plugin, pool) in &mut plugins {
        check_mode(plugin.name(), plugin.modes(), pool.mode)?;
        plugin
            .prepare(&mut prepare)
            .with_context(|| format!("plugin {}: prepare failed", plugin.name()))?;
    }
    worker::serve(plugins, threads, &settings.supervisor).map(ExitCode::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn incompatible_pool_mode_fails_before_startup() {
        assert_eq!(
            check_mode("grpc", &[Mode::Dispatcher], Mode::Worker)
                .unwrap_err()
                .to_string(),
            "grpc.pool.mode = worker: this plugin serves dispatcher"
        );
    }
}

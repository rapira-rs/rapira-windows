use clap::{Args, CommandFactory, Parser, Subcommand};
use extension_api::{ListenAddr, Middleware, PrepareCtx};
use php_sys::Mode;
use rapira_config::{Listen, MiddlewareSettings, Overrides, RunMode, Settings, UnsafeFieldNames};
use rapira_http::{
    Config as HttpConfig, Server as HttpServer, UnsafeFieldNames as HttpUnsafeFieldNames,
};
use rapira_runtime::ExtensionRuntime;
use std::{
    fs::{OpenOptions, read_dir, remove_file},
    path::PathBuf,
    process::ExitCode,
    sync::Arc,
};
use tracing::info;
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_INVALID_PARAMETER, GetLastError, HANDLE, STILL_ACTIVE,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    TerminateProcess,
};

mod logging;
mod pidfile;
mod worker;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

/// PHP application server driven by native extensions.
#[derive(Parser)]
#[command(name = "rapira", version)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Boot the server: start PHP, register extensions, and serve requests.
    Serve(ServeArgs),
}

#[derive(Args)]
struct ServeArgs {
    /// Load settings from a rapira.toml. The flags below override values it sets.
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,

    /// PHP interpreter threads in the single server process. Defaults to the CPU count.
    #[arg(long)]
    processes: Option<usize>,

    /// Run mode: classic, worker, or dispatcher. Overrides `pool.mode`.
    #[arg(long, value_name = "MODE")]
    mode: Option<RunMode>,

    /// Listen on an IP address and port. Use `:port` for all interfaces.
    #[arg(long, value_name = "ADDR")]
    listen: Option<Listen>,

    /// PHP entry script. Overrides `pool.entrypoint` from the configuration file.
    #[arg(value_name = "SCRIPT")]
    script: Option<PathBuf>,
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

enum ProcessStatus {
    OpenError(u32),
    QueryError(u32),
    ExitCode(u32),
}

struct ProcessHandle(HANDLE);

impl Drop for ProcessHandle {
    fn drop(&mut self) {
        // SAFETY: OpenProcess returned this owned handle.
        unsafe { CloseHandle(self.0) };
    }
}

fn process_status(pid: u32) -> ProcessStatus {
    // SAFETY: The process ID is positive. The returned handle stays local and is closed on return.
    // https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-openprocess
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return ProcessStatus::OpenError(unsafe { GetLastError() });
    }
    let handle = ProcessHandle(handle);
    let mut code = 0;
    // SAFETY: The handle has query access and code is writable for the duration of the call.
    // https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-getexitcodeprocess
    if unsafe { GetExitCodeProcess(handle.0, &mut code) } == 0 {
        ProcessStatus::QueryError(unsafe { GetLastError() })
    } else {
        ProcessStatus::ExitCode(code)
    }
}

fn spool_dir_reclaimable(name: &str) -> bool {
    spool_dir_reclaimable_with(name, process_status)
}

fn spool_dir_reclaimable_with(name: &str, probe: impl FnOnce(u32) -> ProcessStatus) -> bool {
    let Some(pid) = name
        .strip_prefix("rapira-spool-")
        .and_then(|p| p.parse::<u32>().ok())
        .filter(|&p| p > 0)
    else {
        return false;
    };
    match probe(pid) {
        ProcessStatus::OpenError(ERROR_INVALID_PARAMETER) => true,
        ProcessStatus::ExitCode(code) => code != STILL_ACTIVE as u32,
        ProcessStatus::OpenError(error) | ProcessStatus::QueryError(error) => {
            tracing::debug!(target: "rapira", "keeping spool directory {name}: process probe failed with error {error}");
            false
        }
    }
}

fn force_exit(code: u8) -> ! {
    // SAFETY: This process can terminate itself. PHP threads may still hold DLL locks.
    // https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-terminateprocess
    unsafe { TerminateProcess(GetCurrentProcess(), u32::from(code)) };
    std::process::abort();
}

fn serve(args: ServeArgs) -> anyhow::Result<ExitCode> {
    let settings: Settings = rapira_config::resolve(
        args.config.as_deref(),
        Overrides {
            listen: args.listen,
            processes: args.processes,
            mode: args.mode,
            entrypoint: args.script,
        },
    )?;

    logging::init(&settings.log)?;
    info!(target: "rapira", "rapira_windows v{} starting", env!("CARGO_PKG_VERSION"));
    let _pidfile = settings
        .supervisor
        .pidfile
        .as_deref()
        .map(pidfile::PidFile::write)
        .transpose()?;

    let mode: Mode = match settings.pool.mode {
        RunMode::Classic => Mode::Classic,
        RunMode::Worker => Mode::Worker(settings.pool.entrypoint.clone()),
        RunMode::Dispatcher => Mode::Dispatcher(settings.pool.entrypoint.clone()),
    };

    let sendfile_root = settings
        .http
        .sendfile_root
        .clone()
        .or_else(|| {
            settings
                .pool
                .entrypoint
                .parent()
                .map(std::path::Path::to_path_buf)
        })
        .ok_or_else(|| anyhow::anyhow!("pool.entrypoint has no parent directory"))?;
    let sendfile_root = std::fs::canonicalize(&sendfile_root).map_err(|error| {
        anyhow::anyhow!(
            "sendfile root {} is not accessible: {error}",
            sendfile_root.display()
        )
    })?;
    php_sys::set_sendfile_root(sendfile_root);

    let mut middleware: Vec<Arc<dyn Middleware>> = Vec::new();
    for mw in &settings.http.middleware {
        match mw {
            MiddlewareSettings::Static(st) => {
                // is_dir() converts every metadata error to false. `metadata` preserves the error code.
                let meta = std::fs::metadata(&st.root).map_err(|e| {
                    anyhow::anyhow!(
                        "http.static.root {} is not accessible: {e}",
                        st.root.display()
                    )
                })?;
                anyhow::ensure!(
                    meta.is_dir(),
                    "http.static.root {} is not a directory",
                    st.root.display()
                );
                info!(target: "rapira", "static files from {}, forbid {:?}", st.root.display(), st.forbid);
                middleware.push(Arc::new(rapira_static_files::StaticFiles::new(
                    st.root.clone(),
                    st.forbid.clone(),
                )));
            }
        }
    }
    let http_cfg: HttpConfig = HttpConfig {
        listen: match settings.http.listen {
            Listen::Tcp(addr) => ListenAddr::Tcp(addr),
        },
        server_name: settings.http.server_name,
        server_port: settings.http.server_port,
        max_body_size: settings.http.max_body_size,
        write_timeout: settings.http.write_timeout,
        drain_grace: settings.supervisor.drain_grace(),
        unsafe_field_names: match settings.http.unsafe_field_names {
            UnsafeFieldNames::Drop => HttpUnsafeFieldNames::Drop,
            UnsafeFieldNames::Reject => HttpUnsafeFieldNames::Reject,
        },
        superglobals: !matches!(mode, Mode::Dispatcher(_)),
        keepalive_timeout: settings.http.keepalive_timeout,
        middleware,
    };
    if matches!(mode, Mode::Dispatcher(_)) {
        std::fs::create_dir_all(&settings.http.uploads.dir).map_err(|e| {
            anyhow::anyhow!(
                "creating http.uploads.dir {}: {e}",
                settings.http.uploads.dir.display()
            )
        })?;
        let probe = settings
            .http
            .uploads
            .dir
            .join(format!(".rapira-probe-{}", std::process::id()));
        let _ = remove_file(&probe);
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&probe)
            .map_err(|e| {
                anyhow::anyhow!(
                    "http.uploads.dir {} is not writable: {e}",
                    settings.http.uploads.dir.display()
                )
            })?;
        let _ = remove_file(&probe);
        match read_dir(&settings.http.uploads.dir) {
            Ok(entries) => {
                for entry in entries.flatten() {
                    if !spool_dir_reclaimable(&entry.file_name().to_string_lossy()) {
                        continue;
                    }
                    let path = entry.path();
                    if let Err(e) = std::fs::remove_dir_all(&path) {
                        tracing::warn!(target: "rapira", "sweeping spool dir {}: {e}", path.display());
                    }
                }
            }
            Err(e) => {
                tracing::warn!(target: "rapira", "listing {} for the spool sweep: {e}", settings.http.uploads.dir.display());
            }
        }
    }
    let upload_limits = rapira_runtime::multipart::Limits {
        dir: settings.http.uploads.dir.clone(),
        max_file_size: settings.http.uploads.max_file_size,
        max_field_size: settings.http.uploads.max_field_size,
        max_files: settings.http.uploads.max_files,
        max_parts: settings.http.uploads.max_parts,
        max_part_headers: settings.http.uploads.max_part_headers,
    };

    let mut host: ExtensionRuntime = ExtensionRuntime::new();
    host.register::<HttpServer>(http_cfg)?;

    let mut prepare_ctx: PrepareCtx = PrepareCtx::new();
    host.prepare_all(&mut prepare_ctx)?;
    let outcome = worker::worker_body(
        host,
        mode,
        settings.pool.entrypoint,
        settings.pool.processes,
        settings.pool.max_requests,
        upload_limits,
        settings.supervisor.process_control_timeout,
    )?;
    if !outcome.joined {
        force_exit(outcome.code);
    }
    Ok(ExitCode::from(outcome.code))
}
#[cfg(test)]
mod tests {
    use super::{ProcessStatus, spool_dir_reclaimable, spool_dir_reclaimable_with};
    use windows_sys::Win32::Foundation::{
        ERROR_ACCESS_DENIED, ERROR_INVALID_PARAMETER, STILL_ACTIVE,
    };

    #[test]
    fn spool_sweep_reclaims_only_dead_pid_dirs() {
        for name in [
            "other-dir",
            "rapira-spool-",
            "rapira-spool-x",
            "rapira-spool--5",
            "rapira-spool-0",
        ] {
            assert!(!spool_dir_reclaimable_with(name, |_| panic!(
                "invalid name reached the process probe"
            )));
        }
        for (status, reclaim) in [
            (ProcessStatus::OpenError(ERROR_INVALID_PARAMETER), true),
            (ProcessStatus::OpenError(ERROR_ACCESS_DENIED), false),
            (ProcessStatus::ExitCode(STILL_ACTIVE as u32), false),
            (ProcessStatus::ExitCode(0), true),
            (ProcessStatus::ExitCode(70), true),
            (ProcessStatus::QueryError(ERROR_INVALID_PARAMETER), false),
            (ProcessStatus::QueryError(ERROR_ACCESS_DENIED), false),
        ] {
            assert_eq!(
                spool_dir_reclaimable_with("rapira-spool-123", |pid| {
                    assert_eq!(pid, 123);
                    status
                }),
                reclaim
            );
        }
        assert!(!spool_dir_reclaimable(&format!(
            "rapira-spool-{}",
            std::process::id()
        )));
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("--help")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id();
        assert!(child.wait().unwrap().success());
        assert!(spool_dir_reclaimable(&format!("rapira-spool-{pid}")));
    }
}

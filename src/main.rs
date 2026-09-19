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
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    path::PathBuf,
    process::ExitCode,
    sync::Arc,
};
use tracing::info;
use windows_sys::Win32::Foundation::{ERROR_INVALID_PARAMETER, GetLastError, STILL_ACTIVE};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    TerminateProcess,
};

mod logging;
mod pidfile;
#[cfg(test)]
mod version_tests;
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

    /// HTTP interpreter threads in the server process. Defaults to the CPU count.
    #[arg(long)]
    processes: Option<usize>,

    /// HTTP run mode: classic, worker, or dispatcher. Overrides `pool.mode`.
    #[arg(long, value_name = "MODE")]
    mode: Option<RunMode>,

    /// HTTP listen address and port. Use `:port` for all interfaces.
    #[arg(long, value_name = "ADDR")]
    listen: Option<Listen>,

    /// HTTP PHP entry script. Overrides `pool.entrypoint` from the configuration file.
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

fn process_status(pid: u32) -> ProcessStatus {
    // SAFETY: The process ID is positive. The returned handle stays local and is closed on return.
    // https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-openprocess
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return ProcessStatus::OpenError(unsafe { GetLastError() });
    }
    // SAFETY: OpenProcess returned this owned process handle.
    let handle = unsafe { OwnedHandle::from_raw_handle(handle) };
    let mut code = 0;
    // SAFETY: The handle has query access and code is writable for the duration of the call.
    // https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-getexitcodeprocess
    if unsafe { GetExitCodeProcess(handle.as_raw_handle(), &mut code) } == 0 {
        ProcessStatus::QueryError(unsafe { GetLastError() })
    } else {
        ProcessStatus::ExitCode(code)
    }
}

fn spool_dir_reclaimable(name: &str) -> bool {
    spool_dir_reclaimable_with(name, std::process::id(), process_status)
}

fn spool_dir_reclaimable_with(
    name: &str,
    current_pid: u32,
    probe: impl FnOnce(u32) -> ProcessStatus,
) -> bool {
    let Some(pid) = name
        .strip_prefix("rapira-spool-")
        .and_then(|p| p.parse::<u32>().ok())
        .filter(|&p| p > 0)
    else {
        return false;
    };
    // Windows can reuse a PID after its process exits. This process has not created its spool directory before the sweep.
    // https://learn.microsoft.com/en-us/dotnet/api/system.diagnostics.process.id#remarks
    if pid == current_pid {
        return true;
    }
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

    let grace = settings.supervisor.process_control_timeout;
    let mut pools = Vec::new();
    if let (Some(http), Some(pool)) = (settings.http, settings.pool) {
        pools.push(http_pool(http, pool, settings.supervisor.drain_grace())?);
    }
    if let Some(grpc) = settings.grpc {
        let registry = Arc::new(rapira_grpc::Registry::load(
            &grpc.protos,
            &grpc.import_paths,
        )?);
        let mode = Mode::GrpcDispatcher {
            entrypoint: grpc.pool.entrypoint.clone(),
            services: rapira_runtime::grpc_services(registry.services()),
        };
        let mut host = ExtensionRuntime::new();
        host.register::<rapira_grpc::Server>(rapira_grpc::Config {
            listen: match grpc.listen {
                Listen::Tcp(addr) => ListenAddr::Tcp(addr),
            },
            registry,
            reflection: grpc.reflection,
            gzip_responses: grpc.gzip_responses,
            max_request_message_size: grpc.max_request_message_size,
            max_response_message_size: grpc.max_response_message_size,
            drain_grace: settings.supervisor.drain_grace(),
            interceptors: Vec::new(),
        })?;
        pools.push(worker::PoolArgs {
            host,
            mode,
            script: grpc.pool.entrypoint,
            processes: grpc.pool.processes,
            max_requests: grpc.pool.max_requests,
            uploads: None,
        });
    }
    let mut prepare_ctx = PrepareCtx::new();
    for pool in &mut pools {
        pool.host.prepare_all(&mut prepare_ctx)?;
    }
    let outcome = worker::worker_body(pools, grace)?;
    if !outcome.joined {
        force_exit(outcome.code);
    }
    Ok(ExitCode::from(outcome.code))
}

fn http_pool(
    http: rapira_config::HttpSettings,
    pool: rapira_config::PoolSettings,
    grace: std::time::Duration,
) -> anyhow::Result<worker::PoolArgs> {
    let mode: Mode = match pool.mode {
        RunMode::Classic => Mode::Classic,
        RunMode::Worker => Mode::Worker(pool.entrypoint.clone()),
        RunMode::Dispatcher => Mode::Dispatcher(pool.entrypoint.clone()),
    };

    let sendfile_root = http
        .sendfile_root
        .clone()
        .or_else(|| pool.entrypoint.parent().map(std::path::Path::to_path_buf))
        .ok_or_else(|| anyhow::anyhow!("pool.entrypoint has no parent directory"))?;
    let sendfile_root = std::fs::canonicalize(&sendfile_root).map_err(|error| {
        anyhow::anyhow!(
            "sendfile root {} is not accessible: {error}",
            sendfile_root.display()
        )
    })?;
    php_sys::set_sendfile_root(sendfile_root);

    let mut middleware: Vec<Arc<dyn Middleware>> = Vec::new();
    for mw in &http.middleware {
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
        listen: match http.listen {
            Listen::Tcp(addr) => ListenAddr::Tcp(addr),
        },
        server_name: http.server_name,
        server_port: http.server_port,
        max_body_size: http.max_body_size,
        write_timeout: http.write_timeout,
        drain_grace: grace,
        unsafe_field_names: match http.unsafe_field_names {
            UnsafeFieldNames::Drop => HttpUnsafeFieldNames::Drop,
            UnsafeFieldNames::Reject => HttpUnsafeFieldNames::Reject,
        },
        superglobals: !matches!(mode, Mode::Dispatcher(_)),
        keepalive_timeout: http.keepalive_timeout,
        middleware,
    };
    if matches!(mode, Mode::Dispatcher(_)) {
        std::fs::create_dir_all(&http.uploads.dir).map_err(|e| {
            anyhow::anyhow!(
                "creating http.uploads.dir {}: {e}",
                http.uploads.dir.display()
            )
        })?;
        let probe = http
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
                    http.uploads.dir.display()
                )
            })?;
        let _ = remove_file(&probe);
        match read_dir(&http.uploads.dir) {
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
                tracing::warn!(target: "rapira", "listing {} for the spool sweep: {e}", http.uploads.dir.display());
            }
        }
    }
    let upload_limits = rapira_runtime::multipart::Limits {
        dir: http.uploads.dir.clone(),
        max_file_size: http.uploads.max_file_size,
        max_field_size: http.uploads.max_field_size,
        max_files: http.uploads.max_files,
        max_parts: http.uploads.max_parts,
        max_part_headers: http.uploads.max_part_headers,
    };

    let mut host: ExtensionRuntime = ExtensionRuntime::new();
    host.register::<HttpServer>(http_cfg)?;

    Ok(worker::PoolArgs {
        host,
        mode,
        script: pool.entrypoint,
        processes: pool.processes,
        max_requests: pool.max_requests,
        uploads: Some(upload_limits),
    })
}

#[cfg(test)]
mod tests {
    use super::{ProcessStatus, spool_dir_reclaimable, spool_dir_reclaimable_with};
    use windows_sys::Win32::Foundation::{
        ERROR_ACCESS_DENIED, ERROR_INVALID_PARAMETER, STILL_ACTIVE,
    };

    #[test]
    fn spool_sweep_reclaims_current_and_dead_pid_dirs() {
        for name in [
            "other-dir",
            "rapira-spool-",
            "rapira-spool-x",
            "rapira-spool--5",
            "rapira-spool-0",
        ] {
            assert!(!spool_dir_reclaimable_with(name, 999, |_| panic!(
                "invalid name reached the process probe"
            )));
        }
        assert!(spool_dir_reclaimable_with(
            "rapira-spool-123",
            123,
            |_| panic!("current PID reached the process probe")
        ));
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
                spool_dir_reclaimable_with("rapira-spool-123", 999, |pid| {
                    assert_eq!(pid, 123);
                    status
                }),
                reclaim
            );
        }
        assert!(spool_dir_reclaimable(&format!(
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

use clap::{Args, CommandFactory, Parser, Subcommand};
use extension_api::{ListenAddr, Middleware, PrepareCtx};
use php_sys::{GrpcMethod, GrpcService, Mode};
use rapira_config::{
    GrpcSettings, HttpSettings, Listen, MiddlewareSettings, PoolSettings, RunMode, Settings,
    SupervisorSettings, UnsafeFieldNames,
};
use rapira_grpc::{Config as GrpcConfig, Schema as GrpcSchema, Server as GrpcServer};
use rapira_http::{
    Config as HttpConfig, Server as HttpServer, UnsafeFieldNames as HttpUnsafeFieldNames,
};
use rapira_runtime::ExtensionRuntime;
use std::{
    fs::{File, OpenOptions, read_dir, remove_file},
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    path::{Path, PathBuf},
    process::ExitCode,
    sync::Arc,
    time::Duration,
};
use tracing::info;
use windows_sys::Win32::Foundation::{ERROR_INVALID_PARAMETER, GetLastError, STILL_ACTIVE};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    TerminateProcess,
};
use worker::{PoolArgs, PoolRun};

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
    /// Path to rapira.toml. Relative paths inside the file resolve against its directory.
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

/// Dispatcher mode only. The host spools file parts here, so the directory must exist and accept a new file. The sweep reclaims the spool directories of servers that are gone.
fn prepare_uploads_dir(dir: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir)
        .map_err(|e| anyhow::anyhow!("creating http.uploads.dir {}: {e}", dir.display()))?;
    let probe = dir.join(format!(".rapira-probe-{}", std::process::id()));
    let _ = remove_file(&probe);
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
        .map_err(|e| anyhow::anyhow!("http.uploads.dir {} is not writable: {e}", dir.display()))?;
    let _ = remove_file(&probe);
    match read_dir(dir) {
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
            tracing::warn!(target: "rapira", "listing {} for the spool sweep: {e}", dir.display());
        }
    }
    Ok(())
}

fn force_exit(code: u8) -> ! {
    // SAFETY: This process can terminate itself. PHP threads may still hold DLL locks.
    // https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-terminateprocess
    unsafe { TerminateProcess(GetCurrentProcess(), u32::from(code)) };
    std::process::abort();
}

/// `table` is the pool table that names the entrypoint, for the error text.
fn check_entrypoint(table: &str, entrypoint: &Path) -> anyhow::Result<()> {
    // The entrypoint is fixed for the lifetime of the pool, so one open at boot covers every request. The open proves read permission. The metadata check rejects a directory.
    let meta = File::open(entrypoint)
        .and_then(|f| f.metadata())
        .map_err(|e| {
            anyhow::anyhow!(
                "{table}.entrypoint {} is not readable: {e}",
                entrypoint.display()
            )
        })?;
    anyhow::ensure!(
        meta.is_file(),
        "{table}.entrypoint {} is not a regular file",
        entrypoint.display()
    );
    Ok(())
}

fn listen_addr(listen: Listen) -> ListenAddr {
    match listen {
        Listen::Tcp(addr) => ListenAddr::Tcp(addr),
    }
}

/// Builds the http pool: its extension host with the bound listener, and its pool settings.
fn http_pool(
    http: HttpSettings,
    supervisor: &SupervisorSettings,
    prepare: &mut PrepareCtx,
) -> anyhow::Result<PoolRun> {
    let entrypoint: PathBuf = http.pool.entrypoint.clone();
    check_entrypoint("http.pool", &entrypoint)?;
    let mode: Mode = match http.pool.mode {
        RunMode::Classic => Mode::Classic,
        RunMode::Worker => Mode::Worker(entrypoint.clone()),
        RunMode::Dispatcher => Mode::Dispatcher(entrypoint),
    };
    let dispatcher: bool = matches!(mode, Mode::Dispatcher(_));

    let sendfile_root = std::fs::canonicalize(&http.sendfile_root).map_err(|error| {
        anyhow::anyhow!(
            "sendfile root {} is not accessible: {error}",
            http.sendfile_root.display()
        )
    })?;
    php_sys::set_sendfile_root(sendfile_root);

    let mut middleware: Vec<Arc<dyn Middleware>> = Vec::new();
    for mw in http.middleware {
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
                    st.root, st.forbid,
                )));
            }
        }
    }
    let http_cfg: HttpConfig = HttpConfig {
        listen: listen_addr(http.listen),
        server_name: http.server_name,
        server_port: http.server_port,
        max_body_size: http.max_body_size,
        write_timeout: http.write_timeout,
        drain_grace: supervisor.drain_grace(),
        unsafe_field_names: match http.unsafe_field_names {
            UnsafeFieldNames::Drop => HttpUnsafeFieldNames::Drop,
            UnsafeFieldNames::Reject => HttpUnsafeFieldNames::Reject,
        },
        superglobals: !dispatcher,
        keepalive_timeout: http.keepalive_timeout,
        middleware,
    };
    let uploads = if dispatcher {
        prepare_uploads_dir(&http.uploads.dir)?;
        Some(rapira_runtime::multipart::Limits {
            dir: http.uploads.dir,
            max_file_size: http.uploads.max_file_size,
            max_field_size: http.uploads.max_field_size,
            max_files: http.uploads.max_files,
            max_parts: http.uploads.max_parts,
            max_part_headers: http.uploads.max_part_headers,
        })
    } else {
        None
    };

    let mut host: ExtensionRuntime = ExtensionRuntime::new();
    host.register::<HttpServer>(http_cfg);
    pool_run("http", &http.pool, mode, host, prepare, uploads)
}

/// Binds the listener of the pool before PHP boots, so a port conflict stops the boot.
fn pool_run(
    name: &'static str,
    pool: &PoolSettings,
    mode: Mode,
    mut host: ExtensionRuntime,
    prepare: &mut PrepareCtx,
    uploads: Option<rapira_runtime::multipart::Limits>,
) -> anyhow::Result<PoolRun> {
    host.prepare_all(prepare)?;
    Ok(PoolRun {
        name,
        host,
        args: PoolArgs {
            mode,
            entrypoint: pool.entrypoint.clone(),
            threads: pool.processes,
            max_requests: pool.max_requests,
            uploads,
        },
    })
}

/// Builds the grpc pool. The schema loads here, before PHP boots, so a bad descriptor set or service name stops the boot with exit code 1.
fn grpc_pool(
    grpc: GrpcSettings,
    supervisor: &SupervisorSettings,
    prepare: &mut PrepareCtx,
) -> anyhow::Result<PoolRun> {
    let entrypoint: PathBuf = grpc.pool.entrypoint.clone();
    check_entrypoint("grpc.pool", &entrypoint)?;
    let schema = Arc::new(GrpcSchema::load(&grpc.descriptor_set, &grpc.services)?);
    let services: Vec<GrpcService> = schema
        .services()
        .iter()
        .map(|s| GrpcService {
            name: s.name.clone(),
            methods: s
                .methods
                .iter()
                .map(|m| GrpcMethod {
                    name: m.name.clone(),
                    input_type: m.input_type.clone(),
                    output_type: m.output_type.clone(),
                    client_streaming: m.client_streaming,
                    server_streaming: m.server_streaming,
                })
                .collect(),
        })
        .collect();

    let mut host: ExtensionRuntime = ExtensionRuntime::new();
    host.register::<GrpcServer>(GrpcConfig {
        listen: listen_addr(grpc.listen),
        schema,
        reflection: grpc.reflection,
        default_timeout: grpc.default_timeout,
        max_timeout: grpc.max_timeout,
        drain_grace: supervisor.drain_grace(),
        keepalive_interval: Duration::from_secs(10),
        keepalive_timeout: Duration::from_secs(10),
    });
    pool_run(
        "grpc",
        &grpc.pool,
        Mode::GrpcDispatcher {
            script: entrypoint,
            services,
        },
        host,
        prepare,
        None,
    )
}

fn serve(args: ServeArgs) -> anyhow::Result<ExitCode> {
    let settings: Settings = rapira_config::resolve(&args.config)?;

    logging::init(&settings.log);
    info!(target: "rapira", "rapira_windows v{} starting", env!("CARGO_PKG_VERSION"));
    let _pidfile = settings
        .supervisor
        .pidfile
        .as_deref()
        .map(pidfile::PidFile::write)
        .transpose()?;

    // One context binds every listener, so it rejects an address that two pools share.
    let mut prepare: PrepareCtx = PrepareCtx::new();
    let http: Option<PoolRun> = settings
        .http
        .map(|http| http_pool(http, &settings.supervisor, &mut prepare))
        .transpose()?;
    let grpc: Option<PoolRun> = settings
        .grpc
        .map(|grpc| grpc_pool(grpc, &settings.supervisor, &mut prepare))
        .transpose()?;
    // The HTTP pool comes first: the process enters its entrypoint directory.
    let pools: Vec<PoolRun> = http.into_iter().chain(grpc).collect();

    let outcome = worker::worker_body(pools, settings.supervisor.process_control_timeout)?;
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

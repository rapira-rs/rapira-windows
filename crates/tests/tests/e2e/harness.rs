use std::collections::BTreeSet;
use std::fs::File;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use windows_sys::Win32::System::Console::{
    AllocConsole, CTRL_BREAK_EVENT, GenerateConsoleCtrlEvent, GetConsoleProcessList,
    SetConsoleCtrlHandler,
};
use windows_sys::Win32::System::Threading::{CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW};
use windows_sys::core::BOOL;

const CONSOLE_PROBE: &str = "RAPIRA_E2E_CONSOLE_PROBE";

/// Connection timeout for a new Windows server process.
pub const BOOT: Duration = Duration::from_secs(30);

/// Verifies console event delivery before server tests use console control.
pub fn assert_console_delivery() {
    static RESULT: OnceLock<Result<(), String>> = OnceLock::new();
    let result = RESULT.get_or_init(|| {
        if let Err(first) = console_delivery_attempt() {
            let mut pid = 0;
            // SAFETY: the output buffer has capacity for the single supplied entry.
            if unsafe { GetConsoleProcessList(&mut pid, 1) } != 0 {
                return Err(first);
            }
            // A redirected test runner can start without a console. Allocate one console and verify that new process groups inherit it.
            if unsafe { AllocConsole() } == 0 {
                return Err(format!(
                    "{first}; AllocConsole failed: {} (no shared console)",
                    io::Error::last_os_error()
                ));
            }
            console_delivery_attempt()
                .map_err(|second| format!("{first}; after AllocConsole: {second}"))?;
        }
        Ok(())
    });
    assert!(
        result.is_ok(),
        "Ctrl+Break delivery preflight failed: {result:?}"
    );
}

fn console_delivery_attempt() -> Result<(), String> {
    let dir = scratch_dir();
    let ready = dir.join("console.ready");
    let log_path = dir.join("console.log");
    let log = File::create(&log_path).map_err(|e| e.to_string())?;
    let mut child = Command::new(std::env::current_exe().map_err(|e| e.to_string())?)
        .args(["--exact", "harness::console_probe_child", "--nocapture"])
        .env(CONSOLE_PROBE, &ready)
        .creation_flags(CREATE_NEW_PROCESS_GROUP)
        .stdout(Stdio::from(log.try_clone().map_err(|e| e.to_string())?))
        .stderr(Stdio::from(log))
        .spawn()
        .map_err(|e| format!("spawn console probe: {e}"))?;
    let result = (|| {
        let deadline = Instant::now() + BOOT;
        while !ready.exists() {
            if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
                return Err(format!(
                    "console probe exited before installing its handler: {status}"
                ));
            }
            if Instant::now() >= deadline {
                return Err("console probe did not install its handler (no shared console)".into());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        // SAFETY: The child is the leader of this process group, and the test has not reaped it.
        if unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, child.id()) } == 0 {
            return Err(format!(
                "GenerateConsoleCtrlEvent failed: {} (no shared console)",
                io::Error::last_os_error()
            ));
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
                return if status.success() {
                    Ok(())
                } else {
                    Err(format!(
                        "console probe did not handle Ctrl+Break: {status} (no shared console)"
                    ))
                };
            }
            if Instant::now() >= deadline {
                return Err(
                    "GenerateConsoleCtrlEvent succeeded but the child did not observe it (no shared console)"
                        .into(),
                );
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    })();
    force_reap(&mut child);
    let result = result.map_err(|error| {
        format!(
            "{error}\n{}",
            std::fs::read_to_string(&log_path).unwrap_or_default()
        )
    });
    let _ = std::fs::remove_dir_all(dir);
    result
}

#[test]
fn console_probe_child() {
    let Some(ready) = std::env::var_os(CONSOLE_PROBE) else {
        return;
    };
    static OBSERVED: AtomicBool = AtomicBool::new(false);
    unsafe extern "system" fn handler(event: u32) -> BOOL {
        if event == CTRL_BREAK_EVENT {
            OBSERVED.store(true, Ordering::Release);
            1
        } else {
            0
        }
    }
    // SAFETY: This handler runs only in the child, has the system ABI, and remains valid until exit.
    if unsafe { SetConsoleCtrlHandler(Some(handler), 1) } == 0 {
        panic!(
            "SetConsoleCtrlHandler failed: {} (no shared console)",
            io::Error::last_os_error()
        );
    }
    std::fs::write(ready, b"handler installed").unwrap();
    let deadline = Instant::now() + BOOT;
    while !OBSERVED.load(Ordering::Acquire) {
        assert!(
            Instant::now() < deadline,
            "Ctrl+Break was not delivered (no shared console)"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

pub fn send_ctrl_break(pid: u32) {
    // SAFETY: Callers retain the `Child` while they send events to its process group.
    if unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, pid) } == 0 {
        panic!(
            "GenerateConsoleCtrlEvent({pid}) failed: {} (no shared console)",
            io::Error::last_os_error()
        );
    }
}

fn force_reap(child: &mut Child) {
    if child.try_wait().ok().flatten().is_none() {
        let _ = Command::new("taskkill")
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .creation_flags(CREATE_NO_WINDOW)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// Isolates PHP module startup and always terminates and reaps a child that does not complete.
pub fn run_isolated_test(name: &str, env_name: &str, value: &str, timeout: Duration) -> String {
    assert_console_delivery();
    let dir = scratch_dir();
    let log = File::create(dir.join("server.log")).unwrap();
    let child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", name, "--nocapture"])
        .env(env_name, value)
        .env_remove(CONSOLE_PROBE)
        .creation_flags(CREATE_NEW_PROCESS_GROUP)
        .stdout(Stdio::from(log.try_clone().unwrap()))
        .stderr(Stdio::from(log))
        .spawn()
        .expect("spawn isolated PHP proof");
    let mut server = Server {
        child,
        addr: "127.0.0.1:0".parse().unwrap(),
        dir,
    };
    let status = server.wait_exit(timeout);
    let log = std::fs::read_to_string(server.dir.join("server.log")).unwrap_or_default();
    assert!(
        status.is_some_and(|status| status.success()),
        "isolated test {name} ({value}) failed or exceeded {timeout:?}: {status:?}\n{log}"
    );
    log
}

/// A running server and the temporary directory that contains its configuration and log.
pub struct Server {
    pub child: Child,
    pub addr: SocketAddr,
    pub dir: PathBuf,
}

impl Server {
    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    pub fn wait_exit(&mut self, timeout: Duration) -> Option<ExitStatus> {
        let end = Instant::now() + timeout;
        loop {
            match self.child.try_wait() {
                Ok(Some(st)) => return Some(st),
                Ok(None) => {}
                Err(_) => return None,
            }
            if Instant::now() >= end {
                return None;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    pub fn try_status(&mut self) -> Option<ExitStatus> {
        self.child.try_wait().ok().flatten()
    }
}

impl Drop for Server {
    /// Reaps only a running child because the system can reuse the process ID of a reaped child.
    fn drop(&mut self) {
        force_reap(&mut self.child);
        if std::thread::panicking() {
            eprintln!("{}", log_tail(&self.dir));
            return;
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// The binary is in the root package, so CARGO_BIN_EXE is not set here. Locate it next to the test binary or use `RAPIRA_BIN`.
fn rapira_bin() -> PathBuf {
    if let Ok(p) = std::env::var("RAPIRA_BIN") {
        return PathBuf::from(p);
    }
    let exe = std::env::current_exe().expect("current_exe");
    let bin = exe
        .parent()
        .and_then(Path::parent)
        .expect("target/<profile> dir")
        .join("rapira.exe");
    assert!(
        bin.exists(),
        "rapira binary not found at {}; build it first (cargo build -p rapira_windows --bin rapira) or set RAPIRA_BIN",
        bin.display()
    );
    bin
}

/// Appends `extra_toml` inside `[pool]`. Another section requires its own header after all pool keys.
pub fn spawn_with_config(fixture: &str, processes: usize, extra_toml: &str) -> Server {
    spawn_with_extras(fixture, processes, "", extra_toml, Some("info"), None)
}

/// Starts an example from the repository `examples` directory.
pub fn spawn_example(name: &str, processes: usize) -> Server {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples")
        .join(name);
    spawn_with_source(&source, name, processes, "", "", Some("info"), None)
}

/// Calls [`spawn_with_config`] with keys in `[http]`, where the final `extra_toml` cannot add values.
pub fn spawn_with_http_extra(fixture: &str, processes: usize, http_extra: &str) -> Server {
    spawn_with_extras(fixture, processes, http_extra, "", Some("info"), None)
}

/// Calls [`spawn_with_config`] without a fixed `RUST_LOG` value, so the `[log]` section controls the filter.
pub fn spawn_without_rust_log(fixture: &str, processes: usize, extra_toml: &str) -> Server {
    spawn_with_extras(fixture, processes, "", extra_toml, None, None)
}

/// Writes php.ini to the child working directory and can set PHPRC to the same path.
pub struct CwdIni<'a> {
    pub contents: &'a str,
    pub via_phprc: bool,
    pub phprc_file: Option<(&'a str, &'a str)>,
}

/// Verifies that the SAPI does not read ini files from the working directory.
pub fn spawn_in_cwd(fixture: &str, processes: usize, php_ini: &str) -> Server {
    let ini = CwdIni {
        contents: php_ini,
        via_phprc: false,
        phprc_file: None,
    };
    spawn_with_extras(fixture, processes, "", "", Some("info"), Some(ini))
}

/// Calls [`spawn_in_cwd`] with PHPRC set to the same directory. It appends `extra_toml` inside `[pool]`, so another section requires its own header and must occur last.
pub fn spawn_with_phprc_and_config(
    fixture: &str,
    processes: usize,
    php_ini: &str,
    extra_toml: &str,
) -> Server {
    let ini = CwdIni {
        contents: php_ini,
        via_phprc: true,
        phprc_file: None,
    };
    spawn_with_extras(fixture, processes, "", extra_toml, Some("info"), Some(ini))
}

/// A PHPRC file has precedence over php.ini in the working directory.
pub fn spawn_with_phprc_file(
    fixture: &str,
    processes: usize,
    cwd_php_ini: &str,
    phprc_relative: &str,
    phprc_contents: &str,
) -> Server {
    let ini = CwdIni {
        contents: cwd_php_ini,
        via_phprc: false,
        phprc_file: Some((phprc_relative, phprc_contents)),
    };
    spawn_with_extras(fixture, processes, "", "", Some("info"), Some(ini))
}

fn spawn_with_extras(
    fixture: &str,
    processes: usize,
    http_extra: &str,
    extra_toml: &str,
    rust_log: Option<&str>,
    cwd_ini: Option<CwdIni<'_>>,
) -> Server {
    spawn_with_source(
        &fixture_path(fixture),
        fixture,
        processes,
        http_extra,
        extra_toml,
        rust_log,
        cwd_ini,
    )
}

fn spawn_with_source(
    source: &Path,
    name: &str,
    processes: usize,
    http_extra: &str,
    extra_toml: &str,
    rust_log: Option<&str>,
    cwd_ini: Option<CwdIni<'_>>,
) -> Server {
    let (dir, entrypoint) = stage_source(source, name);
    if let Some(ini) = &cwd_ini {
        std::fs::write(dir.join("php.ini"), ini.contents).expect("write php.ini");
        if let Some((relative, contents)) = ini.phprc_file {
            let path = dir.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).expect("create PHPRC directory");
            std::fs::write(path, contents).expect("write PHPRC file");
        }
    }
    let mut last_log = String::new();
    for _ in 0..3 {
        let (mut child, addr) = spawn_attempt(
            &dir,
            processes,
            &entrypoint,
            http_extra,
            extra_toml,
            rust_log,
            SpawnOptions {
                cwd_ini: cwd_ini.as_ref(),
                listen: None,
            },
        );
        if wait_for_port(&addr, &mut child, BOOT) {
            return Server { child, addr, dir };
        }
        force_reap(&mut child);
        last_log = log_tail(&dir);
    }
    let _ = std::fs::remove_dir_all(&dir);
    panic!("rapira never accepted a connection after 3 attempts\n{last_log}");
}

/// A temporary directory with a copy of the fixture and its entrypoint name for the configuration.
fn stage_fixture(fixture: &str) -> (PathBuf, String) {
    stage_source(&fixture_path(fixture), fixture)
}

fn stage_source(source: &Path, name: &str) -> (PathBuf, String) {
    let dir = scratch_dir();
    let name = Path::new(name)
        .file_name()
        .unwrap_or_else(|| panic!("source {} has no file name", source.display()));
    std::fs::copy(source, dir.join(name))
        .unwrap_or_else(|e| panic!("copy source {}: {e}", source.display()));
    let entrypoint = name.to_str().expect("fixture name is utf-8").to_owned();
    (dir, entrypoint)
}

#[derive(Default)]
struct SpawnOptions<'a> {
    cwd_ini: Option<&'a CwdIni<'a>>,
    listen: Option<SocketAddr>,
}

/// Starts one process on the requested port. The caller selects the wait method.
fn spawn_attempt(
    dir: &Path,
    processes: usize,
    entrypoint: &str,
    http_extra: &str,
    extra_toml: &str,
    rust_log: Option<&str>,
    options: SpawnOptions<'_>,
) -> (Child, SocketAddr) {
    assert_console_delivery();
    let addr = options
        .listen
        .unwrap_or_else(|| SocketAddr::from(([127, 0, 0, 1], free_port())));
    std::fs::write(
        dir.join("rapira.toml"),
        render_config(addr, processes, entrypoint, http_extra, extra_toml),
    )
    .expect("write config");
    let log = File::create(dir.join("server.log")).expect("create server.log");
    let mut cmd = Command::new(rapira_bin());
    cmd.args(["serve", "--config"]).arg(dir.join("rapira.toml"));
    cmd.env_remove("PHPRC");
    cmd.env("PHP_INI_SCAN_DIR", "");
    if let Some(ini) = options.cwd_ini {
        cmd.current_dir(dir);
        if ini.via_phprc {
            cmd.env("PHPRC", dir);
        }
        if let Some((relative, _)) = ini.phprc_file {
            cmd.env("PHPRC", dir.join(relative));
        }
    }
    match rust_log {
        Some(v) => cmd.env("RUST_LOG", v),
        None => cmd.env_remove("RUST_LOG"),
    };
    let child = cmd
        .creation_flags(CREATE_NEW_PROCESS_GROUP)
        .stdout(Stdio::from(log.try_clone().expect("clone log fd")))
        .stderr(Stdio::from(log))
        .spawn()
        .expect("spawn rapira");
    (child, addr)
}

/// Starts a process that must fail during startup. Waits for exit and returns the status and complete log. With `RUST_BACKTRACE` set, the backtrace after the error can exceed the log tail size.
pub fn spawn_boot_failure(fixture: &str, http_extra: &str) -> (ExitStatus, String) {
    let (dir, entrypoint) = stage_fixture(fixture);
    let (child, addr) = spawn_attempt(
        &dir,
        1,
        &entrypoint,
        http_extra,
        "",
        Some("info"),
        SpawnOptions::default(),
    );
    let mut srv = Server { child, addr, dir };
    let Some(status) = srv.wait_exit(BOOT) else {
        panic!("rapira did not exit");
    };
    let log = std::fs::read_to_string(srv.dir.join("server.log")).unwrap_or_default();
    (status, log)
}

/// Uses the specified address and does not connect to check readiness.
pub fn spawn_on_addr_unchecked(
    fixture: &str,
    processes: usize,
    extra_toml: &str,
    addr: SocketAddr,
) -> Server {
    let (dir, entrypoint) = stage_fixture(fixture);
    let (child, addr) = spawn_attempt(
        &dir,
        processes,
        &entrypoint,
        "",
        extra_toml,
        Some("info"),
        SpawnOptions {
            listen: Some(addr),
            ..Default::default()
        },
    );
    Server { child, addr, dir }
}

fn render_config(
    addr: SocketAddr,
    processes: usize,
    fixture: &str,
    http_extra: &str,
    extra: &str,
) -> String {
    format!(
        "[http]\nlisten = \"{addr}\"\n{http_extra}\n\
         [pool]\nprocesses = {processes}\nentrypoint = \"{fixture}\"\n\n\
         {extra}"
    )
}

/// The child must still be running before a successful connection indicates readiness.
fn wait_for_port(addr: &SocketAddr, child: &mut Child, timeout: Duration) -> bool {
    let end = Instant::now() + timeout;
    while Instant::now() < end {
        if child.try_wait().ok().flatten().is_some() {
            return false;
        }
        if TcpStream::connect_timeout(addr, Duration::from_millis(200)).is_ok() {
            std::thread::sleep(Duration::from_millis(100));
            return child.try_wait().ok().flatten().is_none();
        }
        if child.try_wait().ok().flatten().is_some() {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

/// Sends an HTTP/1.1 GET with `Connection: close`. The connection close delimits the body, so the function does not parse chunked framing or keep-alive state.
pub fn http_get(addr: SocketAddr, path: &str, timeout: Duration) -> io::Result<(u16, Vec<u8>)> {
    http_get_with_headers(addr, path, &[], timeout)
}

/// Calls [`http_get`] with additional request fields in the specified order so repeated names remain separate in the HTTP message.
pub fn http_get_with_headers(
    addr: SocketAddr,
    path: &str,
    fields: &[(&str, &str)],
    timeout: Duration,
) -> io::Result<(u16, Vec<u8>)> {
    parse_status_and_body(&http_get_raw(addr, path, fields, timeout)?)
}

/// Returns the complete response, including the head, for assertions about fields that the client received.
pub fn http_get_raw(
    addr: SocketAddr,
    path: &str,
    fields: &[(&str, &str)],
    timeout: Duration,
) -> io::Result<Vec<u8>> {
    let mut s = TcpStream::connect_timeout(&addr, timeout)?;
    s.set_read_timeout(Some(timeout))?;
    s.set_write_timeout(Some(timeout))?;
    write!(
        s,
        "GET {path} HTTP/1.1\r\nHost: e2e\r\nConnection: close\r\n"
    )?;
    for (name, value) in fields {
        write!(s, "{name}: {value}\r\n")?;
    }
    write!(s, "\r\n")?;
    s.flush()?;
    let mut raw = Vec::new();
    if let Err(e) = s.read_to_end(&mut raw)
        && !(raw.is_empty() && e.kind() == io::ErrorKind::ConnectionReset)
    {
        return Err(e);
    }
    Ok(raw)
}

/// Calls [`http_raw`] and returns unparsed response bytes for assertions about the header block.
pub fn http_raw_bytes(addr: SocketAddr, request: &[u8], timeout: Duration) -> io::Result<Vec<u8>> {
    let mut s = TcpStream::connect_timeout(&addr, timeout)?;
    s.set_read_timeout(Some(timeout))?;
    s.set_write_timeout(Some(timeout))?;
    s.write_all(request)?;
    s.flush()?;
    let mut raw = Vec::new();
    s.read_to_end(&mut raw)?;
    Ok(raw)
}

/// Sends a request supplied by the caller without an implicit Host or Connection field.
pub fn http_raw(addr: SocketAddr, request: &[u8], timeout: Duration) -> io::Result<(u16, Vec<u8>)> {
    let mut s = TcpStream::connect_timeout(&addr, timeout)?;
    s.set_read_timeout(Some(timeout))?;
    s.set_write_timeout(Some(timeout))?;
    s.write_all(request)?;
    s.flush()?;
    let mut raw = Vec::new();
    s.read_to_end(&mut raw)?;
    parse_status_and_body(&raw)
}

/// Sends a request body like [`http_get`]. `content_type` uses bytes because a multipart boundary is opaque data and a field value can contain obs-text.
pub fn http_post(
    addr: SocketAddr,
    path: &str,
    content_type: &[u8],
    body: &[u8],
    timeout: Duration,
) -> io::Result<(u16, Vec<u8>)> {
    let mut s = TcpStream::connect_timeout(&addr, timeout)?;
    s.set_read_timeout(Some(timeout))?;
    s.set_write_timeout(Some(timeout))?;
    let mut req = Vec::new();
    write!(
        req,
        "POST {path} HTTP/1.1\r\nHost: e2e\r\nConnection: close\r\n"
    )?;
    req.extend_from_slice(b"Content-Type: ");
    req.extend_from_slice(content_type);
    write!(req, "\r\nContent-Length: {}\r\n\r\n", body.len())?;
    req.extend_from_slice(body);
    s.write_all(&req)?;
    s.flush()?;
    let mut raw = Vec::new();
    s.read_to_end(&mut raw)?;
    parse_status_and_body(&raw)
}

fn parse_status_and_body(raw: &[u8]) -> io::Result<(u16, Vec<u8>)> {
    if raw.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "closed before any response byte",
        ));
    }
    let head_end = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "no header terminator"))?;
    let status_end = raw
        .windows(2)
        .position(|w| w == b"\r\n")
        .unwrap_or(head_end);
    let status_line = std::str::from_utf8(&raw[..status_end])
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "non-utf8 status line"))?;
    let code = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse::<u16>().ok())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "no status code"))?;
    Ok((code, raw[head_end + 4..].to_vec()))
}

fn ready_threads(log: &str) -> BTreeSet<usize> {
    log.lines()
        .filter_map(|line| {
            let (_, rest) = line.split_once("worker thread ")?;
            let (index, event) = rest.split_once(' ')?;
            event
                .starts_with("ready")
                .then(|| index.parse().ok())
                .flatten()
        })
        .collect()
}

/// Counts distinct interpreter thread indices. Repeated generation-ready lines count once.
pub fn wait_workers(
    srv: &Server,
    deadline: Duration,
    what: &str,
    pred: impl Fn(&BTreeSet<usize>) -> bool,
) -> BTreeSet<usize> {
    let end = Instant::now() + deadline;
    loop {
        let log = std::fs::read_to_string(srv.dir.join("server.log")).unwrap_or_default();
        let ready = ready_threads(&log);
        if pred(&ready) {
            return ready;
        }
        if Instant::now() >= end {
            panic!(
                "timed out after {deadline:?} waiting for {what}\n{}",
                diagnostics(srv)
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

pub fn assert_exit_code(status: Option<ExitStatus>, expected: i32, srv: &Server) {
    match status.and_then(|s| s.code()) {
        Some(code) if code == expected => {}
        Some(code) => panic!(
            "expected exit {expected} [{}], got {code} [{}]\n{}",
            code_name(expected),
            code_name(code),
            diagnostics(srv)
        ),
        None => panic!(
            "expected exit {expected} [{}], but the server has no exit code or is still running\n{}",
            code_name(expected),
            diagnostics(srv)
        ),
    }
}

fn code_name(code: i32) -> String {
    match code {
        0 => "DRAINED/OK".into(),
        1 => "ERROR".into(),
        70 => "BOOT_FAILURE".into(),
        130 => "FORCED".into(),
        other => format!("code {other}"),
    }
}

/// Server process ID, observed interpreter indices, and the end of the log for failures.
pub fn diagnostics(srv: &Server) -> String {
    let log = std::fs::read_to_string(srv.dir.join("server.log")).unwrap_or_default();
    format!(
        "server pid {}, interpreter threads {:?}\n{}",
        srv.pid(),
        ready_threads(&log),
        log_tail(&srv.dir)
    )
}

fn log_tail(dir: &Path) -> String {
    let path = dir.join("server.log");
    let content = std::fs::read_to_string(&path).unwrap_or_default();
    let tail: Vec<&str> = content.lines().rev().take(40).collect();
    let body: String = tail.into_iter().rev().collect::<Vec<_>>().join("\n");
    format!(
        "--- server.log tail ({}) ---\n{body}\n--- end ---",
        path.display()
    )
}

pub fn scratch_dir() -> PathBuf {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("rapira-e2e-{}-{seq}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

pub fn fixture_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/e2e/fixtures")
        .join(name)
}

/// Returns the bundled DLL for `name`. A missing required extension must fail the test.
#[allow(dead_code)] // Supports extension-specific end-to-end cases outside the bundled test suite.
pub fn php_extension(name: &str) -> Option<PathBuf> {
    let stem = name
        .trim_start_matches("php_")
        .trim_end_matches(".dll")
        .trim_end_matches(".so");
    let p = std::env::var_os("PHP_RUNTIME").map(|root| {
        PathBuf::from(root)
            .join("ext")
            .join(format!("php_{stem}.dll"))
    });
    if let Some(p) = &p
        && p.exists()
    {
        return p.clone().into();
    }
    if let Ok(required) = std::env::var("RAPIRA_REQUIRE_EXTS") {
        assert!(
            !required.split(',').any(|e| e.trim() == stem),
            "RAPIRA_REQUIRE_EXTS demands {stem}, but {name} is not at {p:?}"
        );
    }
    None
}

fn free_port() -> u16 {
    let l = TcpListener::bind(("127.0.0.1", 0)).expect("bind ephemeral port");
    l.local_addr().expect("local_addr").port()
}

/// An open connection for incremental reads. The read-to-EOF helpers block until the stream ends.
pub struct Conn {
    s: TcpStream,
    buf: Vec<u8>,
    consumed: usize,
}

impl Conn {
    pub fn open(addr: SocketAddr, timeout: Duration) -> io::Result<Self> {
        let s = TcpStream::connect_timeout(&addr, timeout)?;
        s.set_read_timeout(Some(Duration::from_millis(50)))?;
        s.set_write_timeout(Some(timeout))?;
        Ok(Self {
            s,
            buf: Vec::new(),
            consumed: 0,
        })
    }

    pub fn send(&mut self, request: &[u8]) -> io::Result<()> {
        self.s.write_all(request)?;
        self.s.flush()
    }

    /// Disconnects the client during a response.
    pub fn abandon(self) {
        let _ = self.s.shutdown(std::net::Shutdown::Both);
    }

    fn unread(&self) -> &[u8] {
        &self.buf[self.consumed..]
    }

    /// Reads bytes until `pat` occurs after the consumed position or until `deadline`.
    fn fill_until(&mut self, pat: &[u8], deadline: Duration) -> io::Result<usize> {
        let end = std::time::Instant::now() + deadline;
        loop {
            if let Some(pos) = self.unread().windows(pat.len()).position(|w| w == pat) {
                return Ok(self.consumed + pos);
            }
            if std::time::Instant::now() >= end {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!(
                        "no {pat:?} within {deadline:?}; buffered: {:?}",
                        self.unread()
                    ),
                ));
            }
            let mut chunk = [0u8; 4096];
            match self.s.read(&mut chunk) {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        format!("eof before {pat:?}; buffered: {:?}", self.unread()),
                    ));
                }
                Ok(n) => self.buf.extend_from_slice(&chunk[..n]),
                Err(e)
                    if e.kind() == io::ErrorKind::WouldBlock
                        || e.kind() == io::ErrorKind::TimedOut => {}
                Err(e) => return Err(e),
            }
        }
    }

    /// Reads one head block. Interim heads use separate blocks, so another call reads the next block.
    pub fn read_head(&mut self, deadline: Duration) -> io::Result<(u16, Vec<(String, String)>)> {
        let head_end = self.fill_until(b"\r\n\r\n", deadline)?;
        let head = &self.buf[self.consumed..head_end];
        let text = String::from_utf8_lossy(head).into_owned();
        self.consumed = head_end + 4;
        let mut lines = text.split("\r\n");
        let status = lines
            .next()
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|c| c.parse::<u16>().ok())
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("bad status line in {text:?}"),
                )
            })?;
        let fields = lines
            .filter_map(|l| l.split_once(':'))
            .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_owned()))
            .collect();
        Ok((status, fields))
    }

    /// Waits until the open response contains `pat`, then marks all bytes through the pattern as consumed.
    pub fn read_body_until(&mut self, pat: &[u8], deadline: Duration) -> io::Result<()> {
        let pos = self.fill_until(pat, deadline)?;
        self.consumed = pos + pat.len();
        Ok(())
    }

    /// Read exactly `n` bytes: content-length framing on a connection that stays open.
    pub fn read_n(&mut self, n: usize, deadline: Duration) -> io::Result<Vec<u8>> {
        let end = std::time::Instant::now() + deadline;
        while self.unread().len() < n {
            if std::time::Instant::now() >= end {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("{n} bytes not on the wire; buffered: {:?}", self.unread()),
                ));
            }
            let mut chunk = [0u8; 4096];
            match self.s.read(&mut chunk) {
                Ok(0) => {
                    return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "eof mid body"));
                }
                Ok(m) => self.buf.extend_from_slice(&chunk[..m]),
                Err(e)
                    if e.kind() == io::ErrorKind::WouldBlock
                        || e.kind() == io::ErrorKind::TimedOut => {}
                Err(e) => return Err(e),
            }
        }
        let out = self.unread()[..n].to_vec();
        self.consumed += n;
        Ok(out)
    }

    /// Reads to EOF and returns all unread bytes.
    pub fn read_remaining(&mut self, deadline: Duration) -> io::Result<Vec<u8>> {
        let end = std::time::Instant::now() + deadline;
        loop {
            if std::time::Instant::now() >= end {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "no eof in time"));
            }
            let mut chunk = [0u8; 4096];
            match self.s.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => self.buf.extend_from_slice(&chunk[..n]),
                Err(e)
                    if e.kind() == io::ErrorKind::WouldBlock
                        || e.kind() == io::ErrorKind::TimedOut => {}
                Err(e) => return Err(e),
            }
        }
        let rest = self.unread().to_vec();
        self.consumed = self.buf.len();
        Ok(rest)
    }
}

/// Searches server.log for `needle` until the timeout.
pub fn wait_log_contains(srv: &Server, needle: &str, deadline: Duration) -> bool {
    let path = srv.dir.join("server.log");
    let end = std::time::Instant::now() + deadline;
    loop {
        if std::fs::read_to_string(&path)
            .map(|s| s.contains(needle))
            .unwrap_or(false)
        {
            return true;
        }
        if std::time::Instant::now() >= end {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn readiness_counts_each_thread_once_across_interpreter_generations() {
    let log = "INFO worker thread 0 ready\nINFO worker thread 1 ready\n\
               INFO worker thread 0 recycling\nINFO worker thread 0 ready\n\
               INFO worker thread 10 ready\nINFO worker thread 3 recycling\n";
    assert_eq!(ready_threads(log), [0, 1, 10].into_iter().collect());
}

#[test]
fn an_exited_child_is_checked_before_connecting_to_the_port() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "harness::console_probe_child"])
        .env_remove(CONSOLE_PROBE)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    assert!(child.wait().unwrap().success());
    assert!(!wait_for_port(
        &listener.local_addr().unwrap(),
        &mut child,
        Duration::from_secs(1)
    ));
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );
}

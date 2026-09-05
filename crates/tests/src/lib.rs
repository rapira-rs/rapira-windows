use php_sys::{Frame, Mode, Rapira, Request};
use std::env::set_var;
use std::ffi::{CString, OsStr};
use std::os::raw::{c_char, c_int};
use std::path::{Path, PathBuf};
use std::sync::{self, Mutex, Once, PoisonError};
use tokio::sync::mpsc;

static PHP_LOCK: Mutex<()> = Mutex::new(());
static PHP_ENV: Once = Once::new();
static PHP_LOCK_ASYNC: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[cfg(windows)]
#[link(name = "ucrt", kind = "dylib")]
unsafe extern "C" {
    fn _putenv_s(name: *const c_char, value: *const c_char) -> c_int;

    #[cfg(test)]
    #[link_name = "getenv"]
    fn crt_getenv(name: *const c_char) -> *mut c_char;
}

/// # Safety
/// The caller must serialize environment access while this function runs.
unsafe fn set_php_env(name: &str, value: &OsStr) {
    unsafe { set_var(name, value) };

    #[cfg(windows)]
    {
        let crt_name = CString::new(name).expect("PHP environment name has no NUL");
        let crt_value = CString::new(value.to_string_lossy().as_bytes())
            .expect("PHP environment value has no NUL");
        // PHP reads the scan path with UCRT getenv. Only PHPRC has a Win32 fallback.
        // https://github.com/php/php-src/blob/PHP-8.5/main/php_ini.c#L622-L631
        // https://learn.microsoft.com/en-us/cpp/c-runtime-library/reference/putenv-s-wputenv-s
        let result = unsafe { _putenv_s(crt_name.as_ptr(), crt_value.as_ptr()) };
        assert_eq!(result, 0, "update UCRT environment for {name}");
    }
}

/// Returns the absolute path to a PHP fixture in this crate. The result does not depend on the test working directory.
pub fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(name)
}

pub fn php_lock() -> sync::MutexGuard<'static, ()> {
    init_php_env();
    PHP_LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

pub fn set_phprc(_php: &sync::MutexGuard<'static, ()>, ini: &Path) {
    // SAFETY: PHP_LOCK is held while `_php` lives, so nothing reads the environment concurrently.
    unsafe { set_php_env("PHPRC", ini.as_os_str()) };
}

/// One resident worker serves every request.
pub fn run_worker(name: &str, uris: &[&str]) -> anyhow::Result<Vec<(u16, String)>> {
    let _guard = php_lock();
    let r = Rapira::start(Mode::Worker(fixture(name)))?;
    let h = r.handle();
    let mut out = Vec::with_capacity(uris.len());
    for uri in uris {
        out.push(drain(h.handle_blocking(req(uri, name))?));
    }
    drop(h);
    r.shutdown();
    Ok(out)
}

/// Panics when RAPIRA_REQUIRE_EXTS names an extension that this fixture covers. If CI installs the extension, a skipped test indicates an installation failure.
pub fn assert_skip_allowed(fixture: &str) {
    let Ok(required) = std::env::var("RAPIRA_REQUIRE_EXTS") else {
        return;
    };
    for ext in required.split(',').map(str::trim).filter(|e| !e.is_empty()) {
        assert!(
            !fixture.contains(ext),
            "{fixture} skipped, but RAPIRA_REQUIRE_EXTS demands {ext}"
        );
    }
}

/// Builds a minimal `GET` request for `uri` with `$_SERVER` metadata that identifies `fixture_name`.
pub fn req(uri: &str, fixture_name: &str) -> Request {
    let query = uri.split_once('?').map(|x: (&str, &str)| x.1);
    Request {
        document_root: String::new(),
        https: false,
        method: "GET".into(),
        uri: uri.into(),
        target: None,
        authority: None,
        query: query.unwrap_or("").into(),
        protocol: "HTTP/1.1".into(),
        remote: php_sys::types::Addr::Inet(([127, 0, 0, 1], 8080).into()),
        server: php_sys::types::Addr::Inet(([127, 0, 0, 1], 8080).into()),
        server_name: "localhost".into(),
        server_port: 8080,
        script_filename: fixture(fixture_name),
        script_name: "/index.php".into(),
        headers: vec![],
        server_vars: vec![],
        content_type: None,
        content_length: 0,
        body: php_sys::types::Body::Raw(Box::new(std::io::empty())),
        received_at: None,
        tls: None,
    }
}

/// A response stream collected until `End` or until the producer stops unexpectedly.
#[derive(Default)]
pub struct Resp {
    pub interim: Vec<php_sys::ResponseHead>,
    pub head: Option<php_sys::ResponseHead>,
    pub content_length: Option<u64>,
    pub bodiless: bool,
    pub body: Vec<u8>,
    pub trailers: Vec<(String, Vec<u8>)>,
    pub truncated: bool,
    /// Indicates that an `End` frame arrived. False means that the producer stopped first.
    pub ended: bool,
    /// Number of head frames received. `head` stores only the last frame, so this count detects duplicates.
    pub heads: u32,
}

impl Resp {
    /// A value of 0 means that the producer stopped or completed without a head.
    pub fn status(&self) -> u16 {
        self.head.as_ref().map_or(0, |h| h.status)
    }

    pub fn header(&self, name: &str) -> Option<String> {
        self.head
            .as_ref()?
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| String::from_utf8_lossy(v).into_owned())
    }

    pub fn body_string(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    /// Adds one frame. Returns true when the stream is complete.
    fn fold(&mut self, frame: Frame) -> bool {
        match frame {
            Frame::Interim(h) => self.interim.push(h),
            Frame::Head {
                head,
                content_length,
                bodiless,
                ..
            } => {
                self.heads += 1;
                self.head = Some(head);
                self.content_length = content_length;
                self.bodiless = bodiless;
            }
            Frame::Chunk(b) => self.body.extend_from_slice(&b),
            Frame::File { file, offset, len } => match read_slice(&file, offset, len) {
                Ok(bytes) => self.body.extend_from_slice(&bytes),
                Err(e) => panic!("reading a File frame: {e}"),
            },
            Frame::End {
                trailers,
                truncated,
            } => {
                self.trailers = trailers;
                self.truncated = truncated;
                self.ended = true;
                return true;
            }
        }
        false
    }
}

fn read_slice(file: &std::fs::File, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
    use std::os::windows::fs::FileExt;
    let mut out = vec![0u8; usize::try_from(len).unwrap_or(usize::MAX)];
    let mut done = 0usize;
    while done < out.len() {
        let n = file.seek_read(&mut out[done..], offset + done as u64)?;
        if n == 0 {
            break;
        }
        done += n;
    }
    out.truncate(done);
    Ok(out)
}

/// Polls for the first frame until `deadline`. `None` means that no frame arrived before the deadline. If the producer stops without frames, the function returns `Resp::default()`.
pub fn drain_resp_deadline(
    rx: &mut mpsc::Receiver<Frame>,
    deadline: std::time::Instant,
) -> Option<Resp> {
    loop {
        match rx.try_recv() {
            Ok(frame) => {
                let mut resp = Resp::default();
                if !resp.fold(frame) {
                    while let Some(f) = rx.blocking_recv() {
                        if resp.fold(f) {
                            break;
                        }
                    }
                }
                return Some(resp);
            }
            Err(mpsc::error::TryRecvError::Disconnected) => return Some(Resp::default()),
            Err(mpsc::error::TryRecvError::Empty) => {
                if std::time::Instant::now() >= deadline {
                    return None;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }
    }
}

pub fn drain_resp(mut rx: mpsc::Receiver<Frame>) -> Resp {
    let mut resp = Resp::default();
    while let Some(frame) = rx.blocking_recv() {
        if resp.fold(frame) {
            break;
        }
    }
    resp
}

/// Reads the stream into `(status, body)`. The status is 0 when the stream has no head.
pub fn drain(rx: mpsc::Receiver<Frame>) -> (u16, String) {
    let r = drain_resp(rx);
    (r.status(), r.body_string())
}

pub async fn php_lock_async() -> tokio::sync::MutexGuard<'static, ()> {
    init_php_env();
    PHP_LOCK_ASYNC.lock().await
}

pub async fn drain_resp_async(mut rx: mpsc::Receiver<Frame>) -> Resp {
    let mut resp = Resp::default();
    while let Some(frame) = rx.recv().await {
        if resp.fold(frame) {
            break;
        }
    }
    resp
}

pub async fn drain_async(rx: mpsc::Receiver<Frame>) -> (u16, String) {
    let r = drain_resp_async(rx).await;
    (r.status(), r.body_string())
}

/// Sets PHPRC once, before any `Rapira::start`: https://www.php.net/manual/en/configuration.file.php
fn init_php_env() {
    PHP_ENV.call_once(|| {
        let runtime = PathBuf::from(
            std::env::var_os("PHP_RUNTIME")
                .expect("set PHP_RUNTIME to the matching PHP runtime directory"),
        );
        let extension_dir = runtime.join("ext");
        let extensions = extension_ini(&extension_dir);
        let nonce = std::time::UNIX_EPOCH
            .elapsed()
            .expect("clock is after the Unix epoch")
            .as_nanos();
        let scan_dir =
            std::env::temp_dir().join(format!("rapira-test-ini-{}-{nonce}", std::process::id()));
        std::fs::create_dir(&scan_dir).expect("create process-local PHP INI directory");
        std::fs::write(scan_dir.join("extensions.ini"), extensions)
            .expect("write process-local PHP extension INI");
        // The scan directory adds DLLs while PHPRC selects the configuration for each fixture.
        // https://www.php.net/manual/en/configuration.file.php#configuration.file.scan
        // SAFETY: the Once runs this exactly once, before any Rapira::start / php_module_startup.
        unsafe {
            set_php_env("PHP_INI_SCAN_DIR", scan_dir.as_os_str());
            set_php_env(
                "PHPRC",
                OsStr::new(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/fixtures/ini/shared/php.ini"
                )),
            );
        }
    });
}

fn extension_ini(extension_dir: &Path) -> String {
    let mut ini = format!(
        "extension_dir = \"{}\"\n",
        extension_dir.to_string_lossy().replace('\\', "/")
    );
    // PHP source builds can compile these modules into php8ts.dll, so configure only modules that exist as files.
    for name in [
        "openssl",
        "curl",
        "mbstring",
        "pdo_sqlite",
        "sqlite3",
        "fileinfo",
    ] {
        let dll = format!("php_{name}.dll");
        if extension_dir.join(&dll).is_file() {
            ini.push_str(&format!("extension = {dll}\n"));
        }
    }
    if extension_dir.join("php_opcache.dll").is_file() {
        ini.push_str("zend_extension = php_opcache.dll\n");
    }
    ini
}

#[cfg(all(test, windows))]
mod ini_environment_tests {
    use super::*;
    use std::ffi::CStr;

    #[test]
    fn extension_ini_loads_only_existing_dll_files() {
        let extension_dir = std::env::temp_dir().join(format!(
            "rapira-extension-ini-test-{}-{}",
            std::process::id(),
            std::time::UNIX_EPOCH.elapsed().unwrap().as_nanos()
        ));
        std::fs::create_dir_all(extension_dir.join("php_openssl.dll")).unwrap();
        std::fs::write(extension_dir.join("php_curl.dll"), []).unwrap();
        std::fs::write(extension_dir.join("php_opcache.dll"), []).unwrap();

        let actual = extension_ini(&extension_dir);
        let expected = format!(
            "extension_dir = \"{}\"\nextension = php_curl.dll\nzend_extension = php_opcache.dll\n",
            extension_dir.to_string_lossy().replace('\\', "/")
        );
        assert_eq!(actual, expected);

        std::fs::remove_dir_all(extension_dir).unwrap();
    }

    #[test]
    fn php_ini_scan_directory_reaches_the_c_runtime() {
        let _php = php_lock();
        let expected = std::env::var("PHP_INI_SCAN_DIR").expect("Rust environment has scan dir");
        let actual = unsafe { crt_getenv(c"PHP_INI_SCAN_DIR".as_ptr()) };
        assert!(!actual.is_null(), "PHP C runtime has scan dir");
        let actual = unsafe { CStr::from_ptr(actual) }.to_string_lossy();
        assert_eq!(actual, expected);
    }
}

/// One captured record. Tests check PHP diagnostics by level, target, and text.
#[derive(Debug)]
pub struct Captured {
    pub level: tracing::Level,
    pub target: String,
    pub message: String,
    /// The `context` field is empty when the record has no context. Rapira\log() writes its JSON-encoded context array here.
    pub context: String,
}

static LOG_CAPTURE: Mutex<Vec<Captured>> = Mutex::new(Vec::new());

/// Captured records. The code recovers a poisoned lock so one failed assertion does not cause later tests to fail with `PoisonError`.
pub fn captured() -> sync::MutexGuard<'static, Vec<Captured>> {
    LOG_CAPTURE.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Collects the `message` and `context` fields. It ignores `log.*` metadata fields from records that use the bridge.
#[derive(Default)]
struct Msg {
    message: String,
    context: String,
}

impl Msg {
    fn slot(&mut self, name: &str) -> Option<&mut String> {
        match name {
            "message" => Some(&mut self.message),
            "context" => Some(&mut self.context),
            _ => None,
        }
    }
}

impl tracing::field::Visit for Msg {
    fn record_str(&mut self, f: &tracing::field::Field, v: &str) {
        if let Some(slot) = self.slot(f.name()) {
            *slot = v.to_owned();
        }
    }
    // A `%value` field reaches this function because tracing wraps Display in format_args!, whose Debug implementation calls Display.
    fn record_debug(&mut self, f: &tracing::field::Field, v: &dyn std::fmt::Debug) {
        if let Some(slot) = self.slot(f.name()) {
            *slot = format!("{v:?}");
        }
    }
}

struct CaptureLayer;

impl<S: tracing::Subscriber> tracing_subscriber::layer::Layer<S> for CaptureLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
        let norm = tracing_log::NormalizeEvent::normalized_metadata(event);
        let meta = norm.as_ref().unwrap_or_else(|| event.metadata());
        let mut msg = Msg::default();
        event.record(&mut msg);
        captured().push(Captured {
            level: *meta.level(),
            target: meta.target().to_owned(),
            message: msg.message,
            context: msg.context,
        });
    }
}

/// Returns one `app` target record from `\Rapira\log()` with its level, message, and context JSON.
pub type AppRecord = (tracing::Level, String, String);

/// Runs `script` in classic mode and returns its `app` target records. The fixture must print `logged` last so an incomplete script cannot appear as a script that logged no records.
pub fn app_records(script: &str) -> Vec<AppRecord> {
    let _guard = php_lock();
    init_log_capture();
    captured().clear();

    let r = Rapira::start(Mode::Classic).expect("classic boot");
    let h = r.handle();
    let (status, body) = drain(h.handle_blocking(req("/", script)).expect("dispatch"));
    drop(h);
    r.shutdown();

    assert_eq!(status, 200, "{script} must run clean (body: {body:?})");
    assert!(body.contains("logged"), "{script} ran to the end: {body:?}");

    captured()
        .iter()
        .filter(|c| c.target == "app")
        .map(|c| (c.level, c.message.clone(), c.context.clone()))
        .collect()
}

/// Returns the one `app` record that `script` must create. The count assertion detects any additional record.
pub fn app_record(script: &str) -> AppRecord {
    let records = app_records(script);
    assert_eq!(
        records.len(),
        1,
        "{script} must log exactly one app record (got {records:?})"
    );
    records.into_iter().next().expect("checked above")
}

/// Installs the unfiltered capture subscriber once so `LOG_CAPTURE` receives all records from `tracing` and the `log` facade, including trace records.
pub fn init_log_capture() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        use tracing_subscriber::layer::SubscriberExt;
        use tracing_subscriber::util::SubscriberInitExt;
        let _ = tracing_subscriber::registry().with(CaptureLayer).try_init();
    });
}

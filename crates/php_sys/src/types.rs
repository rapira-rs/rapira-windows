use bytes::Bytes;
use http::header::{AUTHORIZATION, COOKIE, HeaderMap, HeaderName};
use std::ffi::CString;
use std::os::raw::c_int;
use std::path::PathBuf;
use tokio::sync::mpsc::{Sender, error::TrySendError};

#[derive(Debug, Clone)]
pub enum Mode {
    Classic,
    Worker(PathBuf),
    Dispatcher(PathBuf),
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Ok = 0,
    Bailout = 1,
    Throw = 3,
}

impl Outcome {
    pub fn from_c(v: c_int) -> Self {
        match v {
            0 => Self::Ok,
            1 => Self::Bailout,
            3 => Self::Throw,
            _ => Self::Bailout,
        }
    }
}

/// A valid response stream is `Interim* Head? (Chunk|File)* End?`. A channel that closes without `End` indicates that the producer stopped unexpectedly.
pub enum Frame {
    /// Advisory interim head (100-199, never 101).
    Interim(ResponseHead),
    Head {
        head: ResponseHead,
        /// `Some` specifies the framing that the consumer must apply. `None` lets the consumer select the framing, such as chunked framing for HTTP/1.1.
        content_length: Option<u64>,
        /// A 204, 304, 1xx, or `HEAD` response has no body bytes or framing fields in the HTTP message.
        bodiless: bool,
    },
    Chunk(Bytes),
    File {
        file: std::fs::File,
        offset: u64,
        len: u64,
    },
    End {
        trailers: HeaderMap,
        truncated: bool,
    },
}

/// Equivalent to `extension_api::Addr`. php_sys does not depend on extension_api, so the runtime converts between these types.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Addr {
    Inet(std::net::SocketAddr),
    /// `None` identifies an unnamed endpoint, which is usual for a Unix peer.
    Unix(Option<PathBuf>),
}

pub struct ClientCertView {
    pub serial: String,
    pub organization: Option<String>,
    pub fingerprint: String,
}

pub struct TlsView {
    pub version: String,
    pub cipher: String,
    pub alpn: Option<String>,
    pub server_name: Option<String>,
    pub cert: Option<ClientCertView>,
}

/// `unlink` performs all removal. seal calls it during finalization, and `Drop` calls it after an abnormal exit.
pub struct SpooledFile {
    pub path: PathBuf,
}

impl SpooledFile {
    /// Takes the path so a later `Drop` does nothing and does not remove a reused file name.
    pub fn unlink(&mut self) {
        let path = std::mem::take(&mut self.path);
        if path.as_os_str().is_empty() {
            return;
        }
        if let Err(e) = std::fs::remove_file(&path)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(target: "rapira", "removing spool file {}: {e}", path.display());
        }
    }
}

impl Drop for SpooledFile {
    fn drop(&mut self) {
        self.unlink();
    }
}

pub struct FormField {
    pub name: Vec<u8>,
    pub value: Vec<u8>,
    pub headers: Vec<(String, Vec<u8>)>,
}

pub struct UploadedFile {
    pub name: Vec<u8>,
    pub client_filename: Vec<u8>,
    /// Exact content type. An empty value or a value that contains only OWS maps to `None` upstream.
    pub client_media_type: Option<Vec<u8>>,
    pub headers: Vec<(String, Vec<u8>)>,
    pub file: SpooledFile,
    /// Number of bytes written to disk. The FFI boundary requires a 64-bit zend_long.
    pub size: u64,
}

pub struct MultipartBody {
    pub fields: Vec<FormField>,
    pub files: Vec<UploadedFile>,
}

pub enum Body {
    Raw(std::io::Cursor<Vec<u8>>),
    Multipart(MultipartBody),
}

pub struct Request {
    pub method: String,
    pub uri: String,
    /// Request target bytes. An HTTP server can reconstruct them from its parsed URI. `None` uses the bytes from `uri`.
    pub target: Option<Vec<u8>>,
    /// Exact authority value from the client. `None` means that the client did not specify an authority.
    pub authority: Option<Vec<u8>>,
    pub https: bool,
    /// HTTP or CGI value, such as "HTTP/2.0". The exchange view converts it to the API value.
    pub protocol: String,
    pub remote: Addr,
    pub server: Addr,
    /// Configured CGI SERVER_NAME and SERVER_PORT values. URI construction also uses these values as a fallback.
    pub server_name: String,
    pub server_port: u16,
    pub headers: HeaderMap,
    /// First content-type field line, raw bytes.
    pub content_type: Option<Vec<u8>>,
    /// Byte count from the HTTP message. Parsed parts do not determine this value. A value of -1 means that the count is unknown.
    pub content_length: i64,
    pub body: Body,
    /// Unix timestamp in seconds. `None` means that no timestamp is set.
    pub received_at: Option<f64>,
    pub tls: Option<TlsView>,
}

pub struct ResponseHead {
    pub status: u16,
    pub headers: HeaderMap,
}

/// CGI `Status` pseudo-field: php-src passes it as a plain header line.
/// https://www.rfc-editor.org/rfc/rfc3875#section-6.3.3
static STATUS: HeaderName = HeaderName::from_static("status");

fn status_field_code(value: &[u8]) -> Option<u16> {
    let digits: &[u8] = value
        .split(|b| b.is_ascii_whitespace())
        .find(|token| !token.is_empty())?;
    let code: u16 = std::str::from_utf8(digits).ok()?.parse().ok()?;
    (100..=599).contains(&code).then_some(code)
}

/// Stores strings for $_SERVER so `register_server_variables` only borrows them. Its frame cannot contain owned Rust values because a bailout can use longjmp and skip pending drops, which causes undefined behavior.
pub struct ReqC {
    pub method: CString,
    pub query: CString,
    pub uri: CString,
    pub script: CString,
    pub ctype: Option<CString>,
    pub cookie: Option<CString>,
    pub authorization: Option<CString>,
    pub remote_addr: String,
    pub remote_port: String,
    pub server_port: String,
}

fn cgi_addr_strings(addr: &Addr) -> (String, String) {
    match addr {
        Addr::Inet(sa) => (sa.ip().to_string(), sa.port().to_string()),
        // REMOTE_ADDR must contain a host number under RFC 3875 section 4.1.8. Use the loopback address for a Unix peer.
        // https://www.rfc-editor.org/rfc/rfc3875#section-4.1.8
        Addr::Unix(_) => ("127.0.0.1".to_owned(), "0".to_owned()),
    }
}

/// A NUL byte is invalid at the CGI boundary, so the function uses an empty value.
fn cgi_cstring(field: &str, bytes: &[u8]) -> CString {
    CString::new(bytes).unwrap_or_else(|_| {
        tracing::warn!(target: "rapira", "{field} carries a NUL byte; registered empty");
        CString::default()
    })
}

impl ReqC {
    /// Repeated Cookie values use `"; "` as the separator, as required by the php-src parser.
    pub fn build(r: &Request) -> Self {
        let mut joined = Vec::new();
        let cookie: Option<CString> =
            crate::callbacks::joined_field(&r.headers, &COOKIE, &mut joined)
                .map(|v| cgi_cstring("Cookie", v));

        let authorization: Option<CString> = r
            .headers
            .get(AUTHORIZATION)
            .map(|v| cgi_cstring("Authorization", v.as_bytes()));

        let (remote_addr, remote_port) = cgi_addr_strings(&r.remote);

        Self {
            method: cgi_cstring("REQUEST_METHOD", r.method.as_bytes()),
            query: cgi_cstring(
                "QUERY_STRING",
                r.uri.split_once('?').map_or("", |(_, q)| q).as_bytes(),
            ),
            uri: cgi_cstring("REQUEST_URI", r.uri.as_bytes()),
            script: cgi_cstring(
                "SCRIPT_FILENAME",
                crate::context::script().filename.as_bytes(),
            ),
            cookie,
            authorization,
            ctype: r
                .content_type
                .as_deref()
                .map(|s| cgi_cstring("CONTENT_TYPE", s)),
            remote_addr,
            remote_port,
            server_port: r.server_port.to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamState {
    NotSent,
    HeadSent,
    /// Body output started during the handler.
    BodyStreamed,
}

pub struct Context {
    pub req: Request,
    /// `None` in dispatcher mode because that mode has no server context and does not run CGI callbacks.
    pub c: Option<ReqC>,
    pub sender: Option<Sender<Frame>>,
    pub head: Option<ResponseHead>,
    pub body: Vec<u8>,
    pub stream: StreamState,
    pub tearing_down: bool,
}

impl Context {
    pub fn new(req: Request, sender: Sender<Frame>, superglobals: bool) -> Self {
        let c = superglobals.then(|| ReqC::build(&req));
        Self {
            req,
            c,
            sender: Some(sender),
            head: None,
            body: Vec::new(),
            stream: StreamState::NotSent,
            tearing_down: false,
        }
    }

    pub fn is_truncated(&self, errored: bool) -> bool {
        errored && self.stream == StreamState::BodyStreamed
    }

    pub fn commit_head(&mut self, mut status: u16, mut headers: HeaderMap) {
        for value in headers.get_all(&STATUS) {
            match status_field_code(value.as_bytes()) {
                Some(code) => status = code,
                None => tracing::warn!(
                    target: "php",
                    "ignored malformed Status field {:?}; status stays {status}",
                    String::from_utf8_lossy(value.as_bytes())
                ),
            }
        }
        headers.remove(&STATUS);
        self.head = Some(ResponseHead { status, headers });
        self.stream = StreamState::HeadSent;
    }

    /// Seals the buffered response as Head, Chunk, and End. A truncated body has no content length, so the client can detect the incomplete body.
    pub fn finish(&mut self, truncated: bool) {
        let Some(tx) = self.sender.take() else {
            return;
        };
        let body = std::mem::take(&mut self.body);
        if let Some(head) = self.head.take() {
            let bodiless = matches!(head.status, 204 | 304)
                || (100..200).contains(&head.status)
                || self.req.method.eq_ignore_ascii_case("HEAD");
            let content_length = (!bodiless && !truncated).then_some(body.len() as u64);
            send(
                &tx,
                Frame::Head {
                    head,
                    content_length,
                    bodiless,
                },
            );
            if !body.is_empty() {
                send(&tx, Frame::Chunk(body.into()));
            }
        }
        send(
            &tx,
            Frame::End {
                trailers: HeaderMap::new(),
                truncated,
            },
        );
    }
}

/// Blocks only on a full channel: `blocking_send` runs a `block_on` on every call.
fn send(tx: &Sender<Frame>, frame: Frame) {
    if let Err(TrySendError::Full(frame)) = tx.try_send(frame) {
        let _ = tx.blocking_send(frame);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_field_code_takes_the_leading_integer() {
        assert_eq!(status_field_code(b"404"), Some(404));
        assert_eq!(status_field_code(b"404 Not Found"), Some(404));
        assert_eq!(status_field_code(b"  503   "), Some(503));
    }

    /// An invalid or out-of-range value does not change the status. The caller removes the field in all cases.
    #[test]
    fn status_field_code_rejects_implausible_values() {
        assert_eq!(status_field_code(b""), None);
        assert_eq!(status_field_code(b"Not Found"), None);
        assert_eq!(status_field_code(b"99"), None);
        assert_eq!(status_field_code(b"600"), None);
        assert_eq!(status_field_code(b"70000"), None);
    }

    fn head_of(status: u16, headers: &[(&str, &str)]) -> ResponseHead {
        let mut ctx = Context {
            req: Request {
                method: String::new(),
                uri: String::new(),
                target: None,
                authority: None,
                https: false,
                protocol: String::new(),
                remote: Addr::Inet(([127, 0, 0, 1], 8080).into()),
                server: Addr::Inet(([127, 0, 0, 1], 8080).into()),
                server_name: String::new(),
                server_port: 8080,
                headers: HeaderMap::new(),
                content_type: None,
                content_length: -1,
                body: Body::Raw(std::io::Cursor::new(Vec::new())),
                received_at: None,
                tls: None,
            },
            c: None,
            sender: None,
            head: None,
            body: Vec::new(),
            stream: StreamState::NotSent,
            tearing_down: false,
        };
        let headers = headers
            .iter()
            .map(|(k, v)| {
                (
                    HeaderName::from_bytes(k.as_bytes()).unwrap(),
                    http::HeaderValue::from_str(v).unwrap(),
                )
            })
            .collect();
        ctx.commit_head(status, headers);
        ctx.head.expect("commit_head records a head")
    }

    /// The `Status:` field sets the code and is not sent to the client. Matching is case-insensitive because php-src preserves the spelling from the script.
    #[test]
    fn commit_head_consumes_the_status_field() {
        let head = head_of(200, &[("status", "404 Not Found"), ("X-Keep", "kept")]);
        assert_eq!(head.status, 404);
        assert_eq!(head.headers.len(), 1);
        assert!(head.headers.contains_key("x-keep"));
    }

    /// The function removes an invalid `Status:` field without changing the response code. Sending this field would add a literal `Status:` field to the HTTP message.
    #[test]
    fn commit_head_drops_an_unparseable_status_without_changing_the_code() {
        let head = head_of(201, &[("Status", "NotFound")]);
        assert_eq!(head.status, 201);
        assert!(head.headers.is_empty());
    }

    /// RFC 3875 section 6.3.3 makes `Status` the result code of the script, so it overrides the code that php-src recorded. https://www.rfc-editor.org/rfc/rfc3875#section-6.3.3
    #[test]
    fn commit_head_lets_the_status_field_override_the_recorded_code() {
        assert_eq!(head_of(500, &[("Status", "404")]).status, 404);
    }
}

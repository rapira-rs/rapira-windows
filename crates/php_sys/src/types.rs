use bytes::Bytes;
use http::header::{AUTHORIZATION, COOKIE, HeaderMap, HeaderName};
use std::ffi::{CStr, CString};
use std::os::raw::c_int;
use std::path::PathBuf;
use tokio::sync::mpsc::{Sender, error::TrySendError};

#[derive(Debug, Clone)]
pub enum Mode {
    Classic,
    Worker(PathBuf),
    Dispatcher(PathBuf),
    /// Dispatcher mode for a gRPC pool. `get_dispatcher()` gives the gRPC dispatcher.
    GrpcDispatcher {
        script: PathBuf,
        services: Vec<GrpcService>,
    },
}

/// One service of a gRPC pool. `name` is fully qualified.
#[derive(Debug, Clone)]
pub struct GrpcService {
    pub name: String,
    /// In descriptor order.
    pub methods: Vec<GrpcMethod>,
}

/// One method of a `GrpcService`. `name` is the bare method name. The two types are fully qualified message names.
#[derive(Debug, Clone)]
pub struct GrpcMethod {
    pub name: String,
    pub input_type: String,
    pub output_type: String,
    pub client_streaming: bool,
    pub server_streaming: bool,
}

/// `Rapira\Grpc\MethodKind`. Client streaming streams the request, and server streaming streams the response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MethodKind {
    Unary,
    ServerStreaming,
    ClientStreaming,
    BidiStreaming,
}

impl MethodKind {
    pub(crate) fn of(m: &GrpcMethod) -> Self {
        match (m.client_streaming, m.server_streaming) {
            (false, false) => Self::Unary,
            (false, true) => Self::ServerStreaming,
            (true, false) => Self::ClientStreaming,
            (true, true) => Self::BidiStreaming,
        }
    }

    /// Converts the backing value of a case.
    pub(crate) fn from_value(value: &[u8]) -> Option<Self> {
        Some(match value {
            b"unary" => Self::Unary,
            b"server-streaming" => Self::ServerStreaming,
            b"client-streaming" => Self::ClientStreaming,
            b"bidi-streaming" => Self::BidiStreaming,
            _ => return None,
        })
    }

    pub(crate) fn case(self) -> &'static CStr {
        match self {
            Self::Unary => c"Unary",
            Self::ServerStreaming => c"ServerStreaming",
            Self::ClientStreaming => c"ClientStreaming",
            Self::BidiStreaming => c"BidiStreaming",
        }
    }

    pub(crate) fn streams_request(self) -> bool {
        matches!(self, Self::ClientStreaming | Self::BidiStreaming)
    }

    pub(crate) fn streams_response(self) -> bool {
        matches!(self, Self::ServerStreaming | Self::BidiStreaming)
    }
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

/// One unit of work on the worker intake.
pub(crate) enum Unit {
    Http(Box<Context>),
    Grpc(Box<GrpcJob>),
}

/// The gRPC status of a call that a boot-failed worker sheds. https://github.com/grpc/grpc/blob/master/doc/statuscodes.md
const GRPC_UNAVAILABLE: u32 = 14;

impl Unit {
    pub(crate) fn front(&self) -> crate::exchange::Front {
        match self {
            Self::Http(_) => crate::exchange::Front::Http,
            Self::Grpc(_) => crate::exchange::Front::Grpc,
        }
    }

    /// The client left while the unit was queued.
    pub(crate) fn is_closed(&self) -> bool {
        match self {
            Self::Http(job) => job.sender.as_ref().is_some_and(Sender::is_closed),
            Self::Grpc(job) => job.reply.is_closed(),
        }
    }

    /// Answers for a worker that cannot serve: 503 or UNAVAILABLE.
    pub(crate) fn shed(self) {
        match self {
            Self::Http(mut job) => {
                crate::callbacks::send_error_head(&mut job, 503);
                job.finish(false);
            }
            Self::Grpc(job) => {
                let _ = job.reply.send(GrpcOutcome {
                    headers: HeaderMap::new(),
                    trailers: HeaderMap::new(),
                    result: Err(GrpcStatus {
                        code: GRPC_UNAVAILABLE,
                        message: "the worker failed to boot".into(),
                        details: Vec::new(),
                    }),
                });
            }
        }
    }

    /// The HTTP job, for the modes that serve nothing else.
    pub(crate) fn into_http(self) -> Option<Box<Context>> {
        match self {
            Self::Http(job) => Some(job),
            Self::Grpc(_) => None,
        }
    }
}

/// The protocol that the client of a gRPC call used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrpcProtocol {
    Grpc,
    GrpcWeb,
    Connect,
}

/// One unary gRPC call for PHP.
pub struct GrpcRequest {
    /// The full method name, `package.Service/Method`.
    pub method: String,
    pub protocol: GrpcProtocol,
    /// The request metadata as it arrived. PHP gets the application keys only, with `-bin` values decoded.
    pub metadata: HeaderMap,
    /// Unix timestamp after which the outcome is no longer wanted.
    pub deadline: Option<f64>,
    pub remote: Addr,
    /// The binary protobuf encoding of the input message.
    pub message: Bytes,
}

/// The one outcome of a unary call. The metadata is in wire form, so `-bin` values are base64 without padding.
#[derive(Debug, PartialEq)]
pub struct GrpcOutcome {
    pub headers: HeaderMap,
    pub trailers: HeaderMap,
    /// The output message, or the status that the call failed with.
    pub result: Result<Bytes, GrpcStatus>,
}

/// The `google.rpc.Status` triple. `details` holds `google.protobuf.Any` pairs: the type URL and the packed bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrpcStatus {
    pub code: u32,
    pub message: String,
    pub details: Vec<(String, Bytes)>,
}

pub(crate) struct GrpcJob {
    pub(crate) req: GrpcRequest,
    /// Unix timestamp of the enqueue.
    pub(crate) received_at: f64,
    pub(crate) reply: tokio::sync::oneshot::Sender<GrpcOutcome>,
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

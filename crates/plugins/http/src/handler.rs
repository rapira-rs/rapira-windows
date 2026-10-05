use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::task::{Context, Poll};
use std::time::Duration;

use http::header::CONTENT_TYPE;
use http_body_util::BodyExt;
use hyper::body::{Body, Bytes, Frame as BodyFrame, Incoming, SizeHint};
use rapira_sapi::work::{Refused, Sink};
use rapira_sapi::{Addr, Frame, Request};
use tokio::time::timeout;

use crate::check::{self, Rejection};
use crate::request::Peer;
use crate::response::{self, BoxError, error_response, response_headers};
use crate::{Config, Exchange, bridge, multipart, request};

pub(crate) struct Shared {
    pub cfg: Config,
    pub intake: Sink,
    pub inflight: Arc<AtomicUsize>,
}

impl From<Refused> for Rejection {
    fn from(e: Refused) -> Self {
        Self {
            status: match e {
                Refused::Saturated => http::StatusCode::SERVICE_UNAVAILABLE,
                Refused::Stopped => http::StatusCode::INTERNAL_SERVER_ERROR,
            },
            reason: e.to_string(),
        }
    }
}

pub(crate) struct InflightReqCount {
    counter: Arc<AtomicUsize>,
    /// Connection flush count when the last response byte was handed to hyper.
    /// It lives on the shared guard: the body records it and the drain task reads it.
    pub(crate) end_flush: OnceLock<u64>,
}

impl InflightReqCount {
    pub(crate) fn init(counter: &Arc<AtomicUsize>) -> Self {
        counter.fetch_add(1, Ordering::AcqRel);
        Self {
            counter: Arc::clone(counter),
            end_flush: OnceLock::new(),
        }
    }
}

impl Drop for InflightReqCount {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::AcqRel);
    }
}

pub(crate) struct RespBody {
    kind: BodyKind,
    guard: Arc<InflightReqCount>,
    /// Declared body bytes still to pass through, with the connection state that holds the flush count.
    /// Armed by [`respond`] for the final response body.
    transport: Option<(u64, tokio::sync::watch::Receiver<bridge::ConnectionState>)>,
}

#[expect(
    clippy::large_enum_variant,
    reason = "one value per response; a Box would cost an allocation per response"
)]
enum BodyKind {
    Reply(bridge::ReplyBody),
    Empty,
    Boxed(response::Body),
}

fn refused(status: http::StatusCode, req_count: Arc<InflightReqCount>) -> http::Response<RespBody> {
    error_response(status).map(|body| RespBody {
        kind: BodyKind::Boxed(body),
        guard: req_count,
        transport: None,
    })
}

impl Body for RespBody {
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<BodyFrame<Bytes>, BoxError>>> {
        let this = self.get_mut();
        let poll = match &mut this.kind {
            BodyKind::Reply(b) => Pin::new(b).poll_frame(cx),
            BodyKind::Empty => Poll::Ready(None),
            BodyKind::Boxed(b) => Pin::new(b).poll_frame(cx),
        };
        if let Some((remaining, closed)) = &mut this.transport
            && let Poll::Ready(Some(Ok(frame))) = &poll
            && let Some(data) = frame.data_ref()
        {
            *remaining = remaining.saturating_sub(data.len() as u64);
            if *remaining == 0 {
                this.guard.end_flush.get_or_init(|| closed.borrow().flushes);
            }
        }
        poll
    }

    fn is_end_stream(&self) -> bool {
        match &self.kind {
            BodyKind::Reply(_) => false,
            BodyKind::Empty => true,
            BodyKind::Boxed(b) => b.is_end_stream(),
        }
    }

    fn size_hint(&self) -> SizeHint {
        match &self.kind {
            BodyKind::Reply(b) => b.size_hint(),
            BodyKind::Empty => SizeHint::with_exact(0),
            BodyKind::Boxed(b) => b.size_hint(),
        }
    }
}

/// Serves one request of the connection.
pub(crate) async fn respond(
    handler: Arc<Conn>,
    req: http::Request<Incoming>,
) -> http::Response<RespBody> {
    let closed = handler.closed.clone();
    let method = req.method().clone();
    let mut response = handle(handler, req).await;
    if let Some(length) = framed_length(&method, &response) {
        let body = response.body_mut();
        if length == 0 {
            body.guard.end_flush.get_or_init(|| closed.borrow().flushes);
        } else {
            body.transport = Some((length, closed));
        }
    }
    response
}

/// The body length hyper will frame, in the order hyper's h1 encoder (`proto/h1/role.rs`, `Server::encode`) decides it:
/// zero when the method or status forbids a body, else the content-length header, else zero for an ended stream, else the exact size hint.
/// `None` means chunked, which needs no watermark: a chunked PHP reply ends only after PHP sends End.
/// The ended-stream check also survives body combinators that drop the size hint.
fn framed_length(method: &http::Method, response: &http::Response<RespBody>) -> Option<u64> {
    let status = response.status();
    // hyper never polls the body here, whatever the headers say (`Server::can_have_body`).
    if *method == http::Method::HEAD
        || status.is_informational()
        || matches!(
            status,
            http::StatusCode::NO_CONTENT | http::StatusCode::NOT_MODIFIED
        )
    {
        return Some(0);
    }
    response
        .headers()
        .get(http::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok()?.parse().ok())
        .or_else(|| response.body().is_end_stream().then_some(0))
        .or_else(|| response.body().size_hint().exact())
}

async fn handle(handler: Arc<Conn>, req: http::Request<Incoming>) -> http::Response<RespBody> {
    let reqs_counter: Arc<InflightReqCount> =
        Arc::new(InflightReqCount::init(&handler.shared.inflight));
    let received_at: f64 = rapira_sapi::work::now_unix_f64();
    let (mut parts, incoming) = req.into_parts();

    let authority = match check::check_request(
        &mut parts,
        handler.shared.cfg.unsafe_field_names,
        handler.shared.cfg.superglobals,
        handler.shared.cfg.max_body_size,
    ) {
        Ok(authority) => authority,
        Err(rej) => {
            tracing::warn!(target: "http", "rejected: {}", rej.reason);
            return refused(rej.status, reqs_counter);
        }
    };

    if let Some(files) = &handler.shared.cfg.static_files
        && let Some(response) = files.serve(&parts).await
    {
        return response.map(|body| RespBody {
            kind: BodyKind::Boxed(body),
            guard: reqs_counter,
            transport: None,
        });
    }

    let peer: Peer = Peer {
        remote: handler.remote.clone(),
        server: handler.server.clone(),
        https: false,
        received_at,
    };

    serve_php(
        &handler.shared,
        &handler.closed,
        authority,
        reqs_counter,
        &mut parts,
        incoming,
        peer,
    )
    .await
}

/// One HTTP connection.
pub(crate) struct Conn {
    shared: Arc<Shared>,
    closed: tokio::sync::watch::Receiver<bridge::ConnectionState>,
    remote: Addr,
    server: Addr,
}

impl Conn {
    pub(crate) fn new(
        shared: Arc<Shared>,
        remote: Addr,
        server: Addr,
        closed: tokio::sync::watch::Receiver<bridge::ConnectionState>,
    ) -> Arc<Self> {
        Arc::new(Self {
            shared,
            closed,
            remote,
            server,
        })
    }
}

async fn serve_php(
    shared: &Shared,
    closed: &tokio::sync::watch::Receiver<bridge::ConnectionState>,
    authority: Option<Vec<u8>>,
    guard: Arc<InflightReqCount>,
    parts: &mut http::request::Parts,
    body: Incoming,
    peer: Peer,
) -> http::Response<RespBody> {
    let cfg = &shared.cfg;
    let mut body = body;
    let reserve = body.size_hint().lower().min(cfg.max_body_size as u64) as usize;
    let mut collected: Vec<u8> = Vec::with_capacity(reserve);
    loop {
        // hyper only times the head read, so each body frame gets its own progress bound here.
        let frame = match timeout(cfg.keepalive_timeout, body.frame()).await {
            Ok(frame) => frame,
            Err(_) => {
                tracing::debug!(target: "http", "request body stalled past keepalive_timeout");
                return refused(http::StatusCode::REQUEST_TIMEOUT, guard);
            }
        };
        match frame {
            None => break,
            Some(Ok(frame)) => {
                // Non-data frames (request trailers) are dropped: PHP has no surface for them.
                let Ok(data) = frame.into_data() else {
                    continue;
                };
                if collected.len() + data.len() > cfg.max_body_size {
                    tracing::warn!(target: "http", "request body exceeds max_body_size");
                    return refused(http::StatusCode::PAYLOAD_TOO_LARGE, guard);
                }
                collected.extend_from_slice(&data);
            }
            Some(Err(e)) => {
                tracing::debug!(target: "http", "request body read failed: {e}");
                return refused(http::StatusCode::BAD_REQUEST, guard);
            }
        }
    }

    let request = request::build(parts, authority, collected, peer, cfg);
    let mut reply = match submit(shared, request).await {
        Ok(reply) => reply,
        Err(r) => {
            tracing::warn!(target: "http", "rejected before dispatch: {r}");
            return refused(r.status, guard);
        }
    };

    let (status, headers, content_length, bodiless) = loop {
        match reply.recv().await {
            None => {
                tracing::error!(target: "http", "php worker died before a response head");
                return refused(http::StatusCode::BAD_GATEWAY, guard);
            }
            Some(Frame::Interim(head)) => {
                tracing::debug!(target: "http", "dropped interim {}", head.status);
            }
            Some(Frame::Head {
                head,
                content_length,
                bodiless,
            }) => break (head.status, head.headers, content_length, bodiless),
            Some(Frame::End { .. }) => {
                tracing::error!(target: "http", "php produced no response head");
                return refused(http::StatusCode::BAD_GATEWAY, guard);
            }
            Some(Frame::Chunk(_) | Frame::File { .. }) => {
                tracing::warn!(target: "http", "dropped body bytes preceding the response head");
            }
        }
    };

    let status = match http::StatusCode::from_u16(status) {
        Ok(s) if s.as_u16() >= 200 => s,
        _ => {
            // hyper reacts to a service-supplied 1xx by rewriting it to 500 and erroring
            // the connection; a 502 head keeps the connection coherent.
            tracing::error!(
                target: "http",
                "php committed status {status} as final; this plugin cannot forward it - serving 502"
            );
            http::StatusCode::BAD_GATEWAY
        }
    };

    let declared_cl = content_length.filter(|_| !bodiless);

    let kind = if bodiless {
        bridge::spawn_drain(reply, closed.clone(), guard.clone());
        BodyKind::Empty
    } else {
        let staged = if declared_cl.is_some() {
            timeout(Duration::from_millis(10), reply.recv())
                .await
                .ok()
                .flatten()
        } else {
            None
        };
        BodyKind::Reply(bridge::ReplyBody::new(
            reply,
            declared_cl,
            Arc::clone(&guard),
            staged,
            closed.clone(),
        ))
    };

    let mut res = http::Response::new(RespBody {
        kind,
        guard,
        transport: None,
    });
    *res.status_mut() = status;
    *res.headers_mut() = response_headers(headers, declared_cl);
    res
}

/// Both refusals come before dispatch: the multipart parse, then the intake.
async fn submit(
    shared: &Shared,
    request: Request,
) -> Result<tokio::sync::mpsc::Receiver<Frame>, Rejection> {
    let request = parse_multipart(request, shared.cfg.uploads.as_ref()).await?;
    let (exchange, reply) = Exchange::new(
        request,
        shared
            .cfg
            .superglobals
            .then_some(shared.cfg.entrypoint.as_str()),
    );
    shared.intake.submit(Box::new(exchange)).await?;
    Ok(reply)
}

/// Parses a multipart body before submit, so a rejected body never reaches the pending and active counters. `limits` is None outside dispatcher mode.
async fn parse_multipart(
    mut req: Request,
    limits: Option<&multipart::Limits>,
) -> Result<Request, Rejection> {
    let Some(limits) = limits else {
        return Ok(req);
    };
    let rapira_sapi::types::Body::Raw(raw) = &mut req.body else {
        return Ok(req);
    };
    if raw.get_ref().is_empty() {
        return Ok(req);
    }
    // Content-type is a singleton field per RFC 9110 §8.3: with repeated lines the plugin and a PHP consumer could split the body on different boundaries.
    // https://www.rfc-editor.org/rfc/rfc9110#section-8.3
    let lines = req.headers.get_all(CONTENT_TYPE);
    if lines.iter().nth(1).is_some() && lines.iter().any(|v| multipart::is_multipart(v.as_bytes()))
    {
        return Err(Rejection {
            status: http::StatusCode::BAD_REQUEST,
            reason: "repeated content-type field lines with a multipart body".into(),
        });
    }
    let Some(content_type) = req.content_type.as_deref() else {
        return Ok(req);
    };
    if !multipart::is_multipart(content_type) {
        return Ok(req);
    }
    let boundary = multipart::boundary(content_type)?;
    let bytes = std::mem::take(raw.get_mut());
    let limits = limits.clone();
    let parsed = tokio::task::spawn_blocking(move || multipart::parse(&bytes, &boundary, &limits))
        .await
        .map_err(|e| Rejection {
            status: http::StatusCode::INTERNAL_SERVER_ERROR,
            reason: format!("multipart parse task failed: {e}"),
        })?;
    req.body = rapira_sapi::types::Body::Multipart(parsed?);
    Ok(req)
}

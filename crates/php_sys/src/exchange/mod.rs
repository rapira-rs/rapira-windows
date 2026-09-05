pub(crate) use std::{
    cell::Cell,
    ffi::{CStr, CString, c_char, c_int, c_void},
    io::Read,
    path::Path,
    time::Duration,
};

pub(crate) use bytes::Bytes;
pub(crate) use tokio::sync::mpsc::{Sender, error::TrySendError};

pub(crate) use crate::{
    HashPosition, HashTable, IS_ARRAY, IS_STRING, RAPIRA_MODE_DISPATCHER, add_assoc_zval_ex,
    add_next_index_object,
    callbacks::{MAX_BUFFERED_BODY, guard, is_field_value_byte, is_tchar},
    object_init_ex, rapira_array_init, rapira_ce_already_finalized_error,
    rapira_ce_closed_exception, rapira_ce_http_content_length_exceeded_error,
    rapira_ce_http_file_not_sendable_exception, rapira_ce_http_form_field,
    rapira_ce_http_head_already_written_error, rapira_ce_http_head_not_written_error,
    rapira_ce_http_multipart, rapira_ce_http_request, rapira_ce_http_tls,
    rapira_ce_http_uploaded_file, rapira_ce_inet_address, rapira_ce_internal_http_dispatcher,
    rapira_ce_internal_http_dispatcher_info, rapira_ce_internal_http_exchange,
    rapira_ce_no_dispatcher_error, rapira_ce_timeout_exception, rapira_ce_unix_address,
    rapira_ce_work_discarded_exception, rapira_dispatcher_info_obj, rapira_exchange_obj,
    rapira_receive_timed, rapira_receive_untimed,
    scoreboard::{Event, sb_update},
    start::{Pulled, pending_depth, pull_job_try, pull_job_wait},
    types::{
        Addr, Body, Context, FieldLines, FormField, Frame, ResponseHead, TlsView, UploadedFile,
    },
    zend, zend_class_entry, zend_hash_get_current_data_ex, zend_hash_get_current_key_ex,
    zend_hash_internal_pointer_reset_ex, zend_hash_move_forward_ex, zend_object, zend_string, zval,
    zval_add_ref, zval_ptr_dtor,
};

mod headers;
mod receive;
mod request;
mod respond;
mod sendfile;
#[cfg(test)]
mod tests;

pub(crate) use receive::forget_dispatcher;
pub use sendfile::set_sendfile_root;

/// Active variants contain the `Box` pointer so bailout paths can reclaim the unit when free_obj does not run.
#[derive(Clone, Copy)]
enum Unit {
    Idle,
    Handling(*mut ExchangeState),
    Sealed(*mut ExchangeState),
}

#[derive(Clone, Copy)]
struct CycleState {
    unit: Unit,
    closed_seen: bool,
    served: bool,
    /// The cycle has received a unit. A later fatal error is an application failure.
    received: bool,
}

const CYCLE_IDLE: CycleState = CycleState {
    unit: Unit::Idle,
    closed_seen: false,
    served: false,
    received: false,
};

thread_local! {
    static CYCLE: Cell<CycleState> = const { Cell::new(CYCLE_IDLE) };
}

fn update(f: impl FnOnce(&mut CycleState)) {
    let mut c = CYCLE.get();
    f(&mut c);
    CYCLE.set(c);
}

pub(crate) fn cycle_reset() {
    reclaim_current();
    CYCLE.set(CYCLE_IDLE);
}

/// Reclaims a unit when a shutdown or allocation bailout prevents free_obj from receiving it.
pub(crate) fn reclaim_current() {
    if let Unit::Handling(ptr) | Unit::Sealed(ptr) = CYCLE.get().unit {
        update(|c| c.unit = Unit::Idle);
        // SAFETY: The pointer came from Box::into_raw in finish_pull. exchange_drop removes the pointer from tracking before reclamation.
        let st = unsafe { Box::from_raw(ptr) };
        if st.stage != Stage::Finalized {
            sb_update(Event::Handled(true));
        }
        drop(st);
    }
}

pub(crate) fn closed_seen() -> bool {
    CYCLE.get().closed_seen
}

pub(crate) fn note_closed() {
    update(|c| c.closed_seen = true);
}

pub(crate) fn note_received() {
    update(|c| c.received = true);
}

pub(crate) fn note_served() {
    update(|c| c.served = true);
}

pub(crate) fn served_any() -> bool {
    CYCLE.get().served
}

pub(crate) fn received_any() -> bool {
    CYCLE.get().received
}

/// The first head or body write fixes the response head. A body chunk first commits an implicit 200 response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Open,
    HeadCommitted,
    Finalized,
}

/// A committed head that has not been sent. The first body operation sends the bytes.
struct PendingHead {
    status: u16,
    headers: FieldLines,
    body_coded: bool,
}

enum BodyState {
    Raw(Vec<u8>),
    /// seal() unlinks spool files. `Drop` unlinks them after an abnormal exit.
    Multipart {
        fields: Vec<FieldPart>,
        files: Vec<FilePart>,
    },
}

struct FieldPart {
    field: FormField,
    headers: Grouped,
}

struct FilePart {
    upload: UploadedFile,
    path: Vec<u8>,
    headers: Grouped,
}

/// Created during construction so the builder frame contains no owned allocations, as required by the frame rule in zend.rs.
enum AddrOwned {
    Inet {
        ip: String,
        port: u16,
    },
    /// `None` identifies an unnamed endpoint.
    Unix(Option<Vec<u8>>),
}

fn path_bytes(p: &Path) -> Vec<u8> {
    p.to_string_lossy().as_bytes().to_vec()
}

impl AddrOwned {
    fn new(a: &Addr) -> Self {
        match a {
            Addr::Inet(sa) => Self::Inet {
                ip: sa.ip().to_string(),
                port: sa.port(),
            },
            Addr::Unix(p) => Self::Unix(p.as_deref().map(path_bytes).filter(|b| !b.is_empty())),
        }
    }
}

/// Keys are `CString` values because the symbol table prefilter in add_assoc_zval_ex reads one byte after a leading `-`. The string terminator provides this byte.
struct Grouped(Vec<(CString, Vec<Vec<u8>>)>);

impl Grouped {
    fn new(headers: &[(String, Vec<u8>)]) -> Self {
        let mut out: Vec<(CString, Vec<Vec<u8>>)> = Vec::new();
        for (name, value) in headers {
            let nb = name.as_bytes();
            if nb.is_empty() {
                continue;
            }
            match out.iter_mut().find(|(n, _)| n.as_bytes() == nb) {
                Some((_, values)) => values.push(value.clone()),
                None => {
                    let Ok(key) = CString::new(nb) else { continue };
                    out.push((key, vec![value.clone()]));
                }
            }
        }
        Self(out)
    }
}

pub struct ExchangeState {
    // Declare body before job so Rust unlinks spool files before it closes the frame sender.
    body: BodyState,
    job: Box<Context>,
    headers: Grouped,
    uri_abs: String,
    target: Vec<u8>,
    authority: Option<Vec<u8>>,
    /// Required value for `Request::$protocol`, such as HTTP/2. The corresponding CGI value is HTTP/2.0.
    protocol_php: String,
    remote: AddrOwned,
    server: AddrOwned,
    stage: Stage,
    head_sent: bool,
    pending: Option<PendingHead>,
    declared_cl: Option<u64>,
    /// Bytes accepted for `declared_cl`. Responses without a body also count bytes, so a `HEAD` handler receives the same errors.
    sent_body: u64,
    discarded: bool,
    /// For 204, 304, 101, or a `HEAD` request, the function accepts and discards chunks.
    bodiless: bool,
}

impl ExchangeState {
    /// `Err` returns the job with its sender intact. The caller marks the unit as failed.
    fn new(mut job: Box<Context>) -> Result<Self, Box<Context>> {
        let taken = std::mem::replace(&mut job.req.body, Body::Raw(Box::new(std::io::empty())));
        let body = match taken {
            Body::Raw(mut reader) => {
                let mut buf = Vec::new();
                if let Err(e) = reader.read_to_end(&mut buf) {
                    tracing::error!(
                        target: "rapira",
                        "request body read failed for {} {}: {e}",
                        job.req.method, job.req.uri
                    );
                    return Err(job);
                }
                BodyState::Raw(buf)
            }
            Body::Multipart(mb) => BodyState::Multipart {
                fields: mb
                    .fields
                    .into_iter()
                    .map(|f| FieldPart {
                        headers: Grouped::new(&f.headers),
                        field: f,
                    })
                    .collect(),
                files: mb
                    .files
                    .into_iter()
                    .map(|f| FilePart {
                        path: path_bytes(&f.file.path),
                        headers: Grouped::new(&f.headers),
                        upload: f,
                    })
                    .collect(),
            },
        };

        let req = &job.req;
        let headers = Grouped::new(&req.headers);
        let authority = req.authority.clone();
        let target = req
            .target
            .clone()
            .unwrap_or_else(|| req.uri.clone().into_bytes());
        let protocol_php = match req.protocol.as_str() {
            "HTTP/2.0" => "HTTP/2".to_owned(),
            "HTTP/3.0" => "HTTP/3".to_owned(),
            p => p.to_owned(),
        };
        let remote = AddrOwned::new(&req.remote);
        let server = AddrOwned::new(&req.server);

        let scheme = if req.https { "https" } else { "http" };
        let host = match &authority {
            Some(a) => String::from_utf8_lossy(a).into_owned(),
            None => match &req.server {
                Addr::Inet(sa) => sa.to_string(),
                Addr::Unix(_) => format!("{}:{}", req.server_name, req.server_port),
            },
        };
        let path = if req.uri.starts_with('/') {
            req.uri.as_str()
        } else {
            "/"
        };
        let uri_abs = format!("{scheme}://{host}{path}");

        let bodiless = req.method.eq_ignore_ascii_case("HEAD");

        Ok(Self {
            job,
            body,
            headers,
            uri_abs,
            target,
            authority,
            protocol_php,
            remote,
            server,
            stage: Stage::Open,
            head_sent: false,
            pending: None,
            declared_cl: None,
            sent_body: 0,
            discarded: false,
            bodiless,
        })
    }

    fn host_closed(&self) -> bool {
        self.discarded
            || (self.stage != Stage::Finalized
                && self.job.sender.as_ref().is_some_and(Sender::is_closed))
    }
}

/// Gets the enclosing C structure. The C fields occur before `std` in the wrapper.h layout.
unsafe fn exchange_from(obj: *mut zend_object) -> *mut rapira_exchange_obj {
    unsafe {
        obj.byte_sub(std::mem::offset_of!(rapira_exchange_obj, std))
            .cast()
    }
}

unsafe fn info_from(obj: *mut zend_object) -> *mut rapira_dispatcher_info_obj {
    unsafe {
        obj.byte_sub(std::mem::offset_of!(rapira_dispatcher_info_obj, std))
            .cast()
    }
}

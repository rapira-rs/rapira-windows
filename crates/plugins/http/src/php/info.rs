use std::sync::OnceLock;

use rapira_sapi::{rapira_zval_stringl, zend, zval};

struct ServerIdentity {
    name: &'static str,
    port: u16,
}

static SERVER: OnceLock<ServerIdentity> = OnceLock::new();

pub(crate) fn set_server_identity(name: &'static str, port: u16) {
    let _ = SERVER.set(ServerIdentity { name, port });
}

/// # Safety
/// `return_value` is writable. The engine is active on this thread.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rapira_rs_http_server_name(return_value: *mut zval) -> bool {
    let Some(ServerIdentity { name, .. }) = SERVER.get() else {
        return false;
    };
    unsafe {
        rapira_zval_stringl(
            return_value,
            zend::ptr_or_empty(name.as_bytes()),
            name.len(),
        )
    };
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn rapira_rs_http_server_port() -> i64 {
    SERVER.get().map_or(0, |server| i64::from(server.port))
}

#[allow(clippy::all)]
pub mod bindings;

pub mod callbacks;
pub mod classic_worker;
pub mod context;
pub mod diagnostics;
pub mod dispatcher;
pub mod exchange;
pub mod executor;
pub(crate) mod fold;
pub mod handler;
pub mod module;
pub mod quota;
pub mod rapira_worker;
pub mod scoreboard;
pub mod start;
pub mod types;
pub mod values;
pub(crate) mod zend;

use std::ffi::c_int;

pub use bindings::*;
pub use exchange::set_sendfile_root;
pub use handler::{HandleError, RapiraHandle};
pub use quota::PoolHooks;
pub use start::Rapira;
pub use types::{Frame, Mode, Request, ResponseHead};

// Zend SUCCESS and FAILURE values differ between php-src versions, so these constants do not come from the headers.
pub const SUCCESS: c_int = 0;
pub const FAILURE: c_int = -1;

// HASH_KEY_IS_STRING is a #define on 8.4 and an enum constant on 8.5, so it is hardcoded and compared through i64::from at the call sites.
pub const HASH_KEY_IS_STRING: i64 = 1;

// The shims with an `Outcome` result return a C `int`. Call sites decode it with `Outcome::from_c`, which maps unexpected values to `Bailout`.
unsafe extern "C" {
    pub fn rapira_sg() -> *mut sapi_globals_struct;
    pub fn rapira_eg() -> *mut zend_executor_globals;
    pub fn rapira_cg() -> *mut zend_compiler_globals;
    pub fn rapira_pg() -> *mut php_core_globals;
    pub fn rapira_finish_output() -> c_int;
    pub fn rapira_clear_last_error();
    pub fn rapira_request_teardown() -> c_int;
    pub fn rapira_process_init();
    pub fn rapira_tsrmls_cache_update();
    pub fn rapira_thread_init();
    pub fn rapira_thread_disarm();
    pub fn rapira_timer_rearm(timeout: zend_long);
    pub fn rapira_release_temporary_streams();
    // Stores shutdown functions that were registered during startup until the cycle ends (module.c).
    pub fn rapira_stash_boot_shutdown_functions();
    pub fn rapira_request_activate() -> c_int;
    pub fn rapira_request_shutdown() -> c_int;
    // The wall timer is disabled while receive() waits. It is restored with the saved cycle budget when receive() returns a unit (module.c).
    pub fn rapira_receive_untimed();
    pub fn rapira_receive_timed();
    // This C shim in module.c calls rapira_rs_ub_write. It raises the client abort bailout from C so longjmp does not cross the Rust catch_unwind frame.
    pub fn rapira_ub_write(str_: *const std::os::raw::c_char, len: usize) -> usize;

    pub fn rapira_run_handler(fci: *mut zend_fcall_info, fcc: *mut zend_fcall_info_cache) -> c_int;

    pub static mut rapira_module_entry: zend_module_entry;
}

#ifndef RAPIRA_WRAPPER_H
#define RAPIRA_WRAPPER_H

// Enable the overflow built-ins that libclang provides for Bindgen. https://gcc.gnu.org/onlinedocs/gcc/Integer-Overflow-Builtins.html
// Use a plain function pointer with the same size as __vectorcall for Bindgen. The C compiler keeps the PHP calling convention. https://learn.microsoft.com/en-us/cpp/cpp/vectorcall?view=msvc-170
#if defined(_WIN32) && defined(RAPIRA_BINDGEN)
#define PHP_HAVE_BUILTIN_SADDL_OVERFLOW 1
#define PHP_HAVE_BUILTIN_SADDLL_OVERFLOW 1
#define PHP_HAVE_BUILTIN_SSUBL_OVERFLOW 1
#define PHP_HAVE_BUILTIN_SSUBLL_OVERFLOW 1
#define PHP_HAVE_BUILTIN_SMULL_OVERFLOW 1
#define PHP_HAVE_BUILTIN_SMULLL_OVERFLOW 1
#include <Zend/zend_portability.h>
#undef ZEND_FASTCALL
#define ZEND_FASTCALL
#endif

// clang-format off
#include <TSRM/TSRM.h>
#include <Zend/zend.h>
#include <Zend/zend_API.h>
#include <Zend/zend_compile.h>
#include <Zend/zend_globals.h>
#include <Zend/zend_exceptions.h>
#include <Zend/zend_enum.h>
#include <Zend/zend_interfaces.h>
#include <main/php.h>
#include <ext/standard/basic_functions.h>
#include <main/SAPI.h>
#include <main/php_main.h>
#include <main/php_output.h>
#include <main/php_variables.h>
// clang-format on
#ifdef HAVE_PHP_SESSION
#include <ext/session/php_session.h>
#endif
#include <ext/json/php_json.h>
#include <Zend/zend_observer.h>
#include <ext/spl/spl_exceptions.h>
#include <ext/standard/head.h>
#include <main/php_memory_streams.h>
#include <main/php_streams.h>

ZEND_TSRMLS_CACHE_EXTERN()

sapi_globals_struct *rapira_sg(void);
zend_executor_globals *rapira_eg(void);
zend_compiler_globals *rapira_cg(void);
php_core_globals *rapira_pg(void);
void rapira_sapi_startup(sapi_module_struct *sf);
void rapira_tsrmls_cache_update(void);
void rapira_thread_init(void);
void rapira_thread_disarm(void);
void rapira_timer_rearm(zend_long timeout);
void rapira_dispatcher_thread_init(void);
void rapira_process_init(void);
void rapira_release_temporary_streams(void);
void rapira_stash_boot_shutdown_functions(void);
int rapira_request_activate(void);
int rapira_request_shutdown(void);
size_t rapira_ub_write(const char *str, size_t len);
// Provide Rust shims because only macros or inline functions define array_init_size and smart_str_free.
void rapira_array_init(zval *zv, uint32_t size);
void rapira_smart_str_free(smart_str *s);

#if PHP_VERSION_ID >= 80500
void rapira_zend_hash_internal_pointer_reset_ex(const HashTable *ht, HashPosition *pos);
zval *rapira_zend_hash_get_current_data_ex(const HashTable *ht, const HashPosition *pos);
zend_hash_key_type rapira_zend_hash_get_current_key_ex(const HashTable *ht, zend_string **str_index, zend_ulong *num_index, const HashPosition *pos);
zend_result rapira_zend_hash_move_forward_ex(const HashTable *ht, HashPosition *pos);
#else
void rapira_zend_hash_internal_pointer_reset_ex(HashTable *ht, HashPosition *pos);
zval *rapira_zend_hash_get_current_data_ex(HashTable *ht, HashPosition *pos);
int rapira_zend_hash_get_current_key_ex(const HashTable *ht, zend_string **str_index, zend_ulong *num_index, const HashPosition *pos);
zend_result rapira_zend_hash_move_forward_ex(HashTable *ht, HashPosition *pos);
#endif
zval *rapira_zend_hash_index_update(HashTable *ht, zend_ulong index, zval *value);
zval *rapira_zend_hash_str_update(HashTable *ht, const char *key, size_t len, zval *value);
bool rapira_instanceof_function_slow(const zend_class_entry *instance_ce, const zend_class_entry *ce);
typedef zend_string *(*rapira_string_init_interned_fn)(const char *str, size_t size, bool permanent);
extern rapira_string_init_interned_fn rapira_zend_string_init_interned;

// Keep these values the same as Mode in types.rs and start_pool in start.rs.
enum {
    RAPIRA_MODE_CLASSIC = 0,
    RAPIRA_MODE_WORKER = 1,
    RAPIRA_MODE_DISPATCHER = 2,
};
extern int rapira_mode;

// Keep these values the same as HandleAction in rapira_worker.rs.
enum {
    RAPIRA_HANDLE_STOP = 0,
    RAPIRA_HANDLE_CONTINUE = 1,
    RAPIRA_HANDLE_RECYCLE = 2,
};

// Place the C fields before zend_object so Bindgen names the fields for Rust. This layout removes a fixed offset from the Rust code. https://www.zend.com/resources/php-extensions/embedding-c-data-into-php-objects
typedef struct {
    void *job; // Rust owns this Box<ExchangeState>. Set it to NULL after release.
    zval request; // This field caches Rapira\Http\Request and contains IS_UNDEF before getRequest().
    zend_object std;
} rapira_exchange_obj;

typedef struct {
    zend_long pending;
    zend_long active;
    zend_object std;
} rapira_dispatcher_info_obj;

// Rust accesses these class entries. rapira_register_classes sets them in MINIT before PHP creates an object.
extern zend_class_entry *rapira_ce_log_level;
extern zend_class_entry *rapira_ce_mode;
extern zend_class_entry *rapira_ce_closed_exception;
extern zend_class_entry *rapira_ce_timeout_exception;
extern zend_class_entry *rapira_ce_work_discarded_exception;
extern zend_class_entry *rapira_ce_no_dispatcher_error;
extern zend_class_entry *rapira_ce_not_in_worker_mode_error;
extern zend_class_entry *rapira_ce_already_finalized_error;
extern zend_class_entry *rapira_ce_http_tls;
extern zend_class_entry *rapira_ce_http_multipart;
extern zend_class_entry *rapira_ce_internal_http_dispatcher;
extern zend_class_entry *rapira_ce_inet_address;
extern zend_class_entry *rapira_ce_unix_address;
extern zend_class_entry *rapira_ce_internal_http_exchange;
extern zend_class_entry *rapira_ce_internal_http_dispatcher_info;
extern zend_class_entry *rapira_ce_http_head_already_written_error;
extern zend_class_entry *rapira_ce_http_head_not_written_error;
extern zend_class_entry *rapira_ce_http_content_length_exceeded_error;
extern zend_class_entry *rapira_ce_http_file_not_sendable_exception;
extern zend_class_entry *rapira_ce_http_form_field;
extern zend_class_entry *rapira_ce_http_uploaded_file;
extern zend_class_entry *rapira_ce_http_request;

// Return PHP_VERSION_ID from the compile-time headers. It can differ from php_version_id() in a replacement libphp.
unsigned int rapira_headers_php_version_id(void);

void rapira_receive_untimed(void);
void rapira_receive_timed(void);

#endif

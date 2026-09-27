#include "rapira_sapi.h"

#include <Zend/zend_smart_str.h>

ZEND_TSRMLS_CACHE_DEFINE()

static void (*rapira_log_message)(const char *message, int priority);

static void rapira_log_message_with_core_priority(const char *message, int priority) {
    // Windows PHP has different syslog priorities.
    // https://github.com/php/php-src/blob/PHP-8.5/win32/syslog.h
    if (priority == LOG_ERR) {
        priority = 3;
    } else if (priority == LOG_WARNING) {
        priority = 4;
    }
    if (rapira_log_message) {
        rapira_log_message(message, priority);
    }
}

void rapira_sapi_startup(sapi_module_struct *sf) {
    rapira_log_message = sf->log_message;
    sf->log_message = rapira_log_message_with_core_priority;
    sapi_startup(sf);
}

unsigned int rapira_headers_php_version_id(void) { return PHP_VERSION_ID; }
void rapira_tsrmls_cache_update(void) { ZEND_TSRMLS_CACHE_UPDATE(); }
const char *rapira_getcwd(void) {
    ZEND_TLS char path[MAXPATHLEN];
    return VCWD_GETCWD(path, sizeof(path));
}
int rapira_chdir(const char *directory) {
    volatile int result = FAILURE;
    zend_try { result = VCWD_CHDIR(directory); }
    zend_catch { result = FAILURE; }
    zend_end_try();
    return result;
}

sapi_globals_struct *rapira_sg(void) {
    return TSRMG_FAST_BULK(sapi_globals_offset, sapi_globals_struct *);
}

zend_executor_globals *rapira_eg(void) {
    return TSRMG_FAST_BULK(executor_globals_offset, zend_executor_globals *);
}

zend_compiler_globals *rapira_cg(void) {
    return TSRMG_FAST_BULK(compiler_globals_offset, zend_compiler_globals *);
}

php_core_globals *rapira_pg(void) {
    return TSRMG_FAST_BULK(core_globals_offset, php_core_globals *);
}

void rapira_array_init(zval *zv, uint32_t size) {
    array_init_size(zv, size);
}

void rapira_smart_str_free(smart_str *s) {
    smart_str_free(s);
}

zval *rapira_symtable_str_find(HashTable *ht, const char *str, size_t len) {
    return zend_symtable_str_find(ht, str, len);
}

bool rapira_array_is_list(HashTable *ht) {
    return zend_array_is_list(ht);
}

void rapira_zval_enum_case(zval *dst, zend_class_entry *ce, const char *name) {
    ZVAL_OBJ_COPY(dst, zend_enum_get_case_cstr(ce, name));
}

void rapira_zval_stringl(zval *zv, const char *s, size_t len) {
    ZVAL_STRINGL(zv, s, len);
}

void rapira_register_known_stringl(const char *name, size_t name_len,
                                   const char *val, size_t val_len,
                                   zval *track_vars_array) {
    zval value;
    ZVAL_STRINGL_FAST(&value, val, val_len);
    php_register_known_variable(name, name_len, &value, track_vars_array);
}

void rapira_init_call_stack(void) {
#ifdef ZEND_CHECK_STACK_LIMIT
    zend_call_stack_init();
#endif
}

// C preserves the PHP vectorcall ABI at the Rust boundary.
// https://learn.microsoft.com/en-us/cpp/cpp/vectorcall?view=msvc-170
#if PHP_VERSION_ID >= 80500
void rapira_zend_hash_internal_pointer_reset_ex(const HashTable *ht, HashPosition *pos) {
    zend_hash_internal_pointer_reset_ex(ht, pos);
}
zval *rapira_zend_hash_get_current_data_ex(const HashTable *ht, const HashPosition *pos) {
    return zend_hash_get_current_data_ex(ht, pos);
}
zend_hash_key_type rapira_zend_hash_get_current_key_ex(const HashTable *ht, zend_string **str_index, zend_ulong *num_index, const HashPosition *pos) {
    return zend_hash_get_current_key_ex(ht, str_index, num_index, pos);
}
zend_result rapira_zend_hash_move_forward_ex(const HashTable *ht, HashPosition *pos) {
    return zend_hash_move_forward_ex(ht, pos);
}
#else
void rapira_zend_hash_internal_pointer_reset_ex(HashTable *ht, HashPosition *pos) {
    zend_hash_internal_pointer_reset_ex(ht, pos);
}
zval *rapira_zend_hash_get_current_data_ex(HashTable *ht, const HashPosition *pos) {
    return zend_hash_get_current_data_ex(ht, pos);
}
int rapira_zend_hash_get_current_key_ex(const HashTable *ht, zend_string **str_index, zend_ulong *num_index, const HashPosition *pos) {
    return zend_hash_get_current_key_ex(ht, str_index, num_index, pos);
}
zend_result rapira_zend_hash_move_forward_ex(HashTable *ht, HashPosition *pos) {
    return zend_hash_move_forward_ex(ht, pos);
}
#endif
zval *rapira_zend_hash_index_update(HashTable *ht, zend_ulong index, zval *value) {
    return zend_hash_index_update(ht, index, value);
}
zval *rapira_zend_hash_str_update(HashTable *ht, const char *key, size_t len, zval *value) {
    return zend_hash_str_update(ht, key, len, value);
}
zval *rapira_zend_hash_str_find(const HashTable *ht, const char *key, size_t len) {
    return zend_hash_str_find(ht, key, len);
}
void rapira_zend_hash_extend(HashTable *ht, uint32_t size, bool packed) {
    zend_hash_extend(ht, size, packed);
}
bool rapira_instanceof_function_slow(const zend_class_entry *instance_ce, const zend_class_entry *ce) {
    return instanceof_function_slow(instance_ce, ce);
}
static zend_string *rapira_string_init_interned(const char *str, size_t size, bool permanent) {
    return zend_string_init_interned(str, size, permanent);
}
rapira_string_init_interned_fn rapira_zend_string_init_interned = rapira_string_init_interned;

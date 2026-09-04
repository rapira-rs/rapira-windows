#include "wrapper.h"

#include <Zend/zend_smart_str.h>

#if defined(PHP_WIN32) && defined(ZTS)
ZEND_TSRMLS_CACHE_DEFINE()
#endif

static void (*rapira_log_message)(const char *message, int syslog_type_int);

static void rapira_log_message_with_core_priority(const char *message, int syslog_type_int) {
    int priority = syslog_type_int;

    // Windows PHP maps error to 4 and warning to 5. https://github.com/php/php-src/blob/PHP-8.5/win32/syslog.h
    if (priority == LOG_ERR) {
        priority = 3;
    } else if (priority == LOG_WARNING) {
        priority = 4;
    }
    if (rapira_log_message != NULL) {
        rapira_log_message(message, priority);
    }
}

void rapira_sapi_startup(sapi_module_struct *sf) {
    rapira_log_message = sf->log_message;
    sf->log_message = rapira_log_message_with_core_priority;
    sapi_startup(sf);
}

unsigned int rapira_headers_php_version_id(void) {
    return PHP_VERSION_ID;
}

// ts_resource_ex must initialize this thread before it accesses PHP globals. https://github.com/php/php-src/blob/PHP-8.5/TSRM/TSRM.h
sapi_globals_struct *rapira_sg(void) {
#ifdef ZTS
    return TSRMG_FAST_BULK(sapi_globals_offset, sapi_globals_struct *);
#else
    return &sapi_globals;
#endif
}

zend_executor_globals *rapira_eg(void) {
#ifdef ZTS
    return TSRMG_FAST_BULK(executor_globals_offset, zend_executor_globals *);
#else
    return &executor_globals;
#endif
}

zend_compiler_globals *rapira_cg(void) {
#ifdef ZTS
    return TSRMG_FAST_BULK(compiler_globals_offset, zend_compiler_globals *);
#else
    return &compiler_globals;
#endif
}

php_core_globals *rapira_pg(void) {
#ifdef ZTS
    return TSRMG_FAST_BULK(core_globals_offset, php_core_globals *);
#else
    return &core_globals;
#endif
}

void rapira_array_init(zval *zv, uint32_t size) {
    array_init_size(zv, size);
}

void rapira_smart_str_free(smart_str *s) {
    smart_str_free(s);
}

// These C functions preserve the PHP __vectorcall ABI. https://learn.microsoft.com/en-us/cpp/cpp/vectorcall?view=msvc-170
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

zval *rapira_zend_hash_get_current_data_ex(HashTable *ht, HashPosition *pos) {
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

bool rapira_instanceof_function_slow(const zend_class_entry *instance_ce, const zend_class_entry *ce) {
    return instanceof_function_slow(instance_ce, ce);
}

static zend_string *rapira_string_init_interned(const char *str, size_t size, bool permanent) {
    return zend_string_init_interned(str, size, permanent);
}

rapira_string_init_interned_fn rapira_zend_string_init_interned = rapira_string_init_interned;

void rapira_tsrmls_cache_update(void) {
#if defined(PHP_WIN32) && defined(ZTS)
    ZEND_TSRMLS_CACHE_UPDATE();
#endif
}

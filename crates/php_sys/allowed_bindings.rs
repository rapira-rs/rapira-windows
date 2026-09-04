bind! {
    sapi_module_struct, sapi_headers_struct, sapi_header_struct, sapi_request_info,
    sapi_globals_struct, zend_executor_globals, php_core_globals, zend_compiler_globals,
    zend_file_handle, zend_module_entry, zend_string, zval, HashTable, zend_long,
    zend_fcall_info, zend_fcall_info_cache,
    sapi_startup, sapi_shutdown, sapi_activate, php_module_startup, php_module_shutdown, php_request_startup, php_request_shutdown,
    php_tsrm_startup_ex, tsrm_shutdown, ts_resource_ex, ts_free_thread,
    php_execute_script, zend_error, zend_stream_init_filename, zend_destroy_file_handle,
    php_register_variable_safe, php_output_deactivate, rapira_mode, RAPIRA_MODE_CLASSIC, RAPIRA_MODE_WORKER,
    RAPIRA_MODE_DISPATCHER,
    // Both parts of the linked libphp version check.
    rapira_headers_php_version_id, php_version_id,
    // Embedded object layouts. wrapper.h contains the authoritative definitions.
    rapira_exchange_obj, rapira_dispatcher_info_obj,
    // Class entry globals that MINIT writes and the Rust builder reads.
    rapira_ce_http_request, rapira_ce_http_multipart, rapira_ce_http_form_field,
    rapira_ce_http_uploaded_file, rapira_ce_http_tls, rapira_ce_inet_address,
    rapira_ce_unix_address, rapira_ce_already_finalized_error,
    rapira_ce_http_head_already_written_error, rapira_ce_internal_http_exchange,
    rapira_ce_internal_http_dispatcher, rapira_ce_internal_http_dispatcher_info,
    rapira_ce_timeout_exception, rapira_ce_closed_exception,
    rapira_ce_no_dispatcher_error,
    zend_argument_value_error, zend_argument_type_error,
    rapira_ce_work_discarded_exception, rapira_ce_http_content_length_exceeded_error,
    rapira_ce_http_head_not_written_error, rapira_ce_http_file_not_sendable_exception,
    // The FOREACH macros exist only in headers, so array iteration uses the exported position API.
    zend_hash_internal_pointer_reset_ex, zend_hash_get_current_key_ex,
    zend_hash_get_current_data_ex, zend_hash_move_forward_ex, HashPosition,
    IS_NULL, IS_ARRAY, IS_REFERENCE,
    // zend_throw_error and zend_value_error are variadic functions that use a fixed format here.
    zend_throw_error, zend_value_error, zend_throw_exception,
    // instanceof_function is inline, and PHP exports only its slow path.
    zend_update_property_str, instanceof_function_slow, zend_zval_value_name, IS_OBJECT,
    // add_assoc_str, add_index_zval, and smart_str_free are inline. These functions and the shim provide the exported operations.
    zend_read_property, zend_get_exception_base, zend_ce_throwable, php_json_encode,
    smart_str, PHP_JSON_PARTIAL_OUTPUT_ON_ERROR, add_assoc_stringl_ex,
    zend_hash_index_update, rapira_smart_str_free,
    // zend_update_property functions change EG(fake_scope) to initialize readonly properties.
    object_init_ex, zend_update_property, zend_update_property_stringl,
    zend_update_property_long, zend_update_property_double, zend_update_property_null,
    // The rapira_array_init shim in wrapper.c provides the array_init_size macro operation.
    rapira_array_init,
    // zend_symtable_str_update is inline. add_assoc_zval_ex is its exported caller in zend_API.c.
    add_assoc_zval_ex, add_next_index_stringl, add_next_index_object,
    zval_add_ref,
    zend_object, zend_class_entry, zval_ptr_dtor, // zval_ptr_dtor_nogc is inline, so use zval_ptr_dtor.
    // zend_string_init is inline. The exported interner function pointer supports strings created during startup.
    zend_hash_str_update, rapira_zend_string_init_interned,
    php_default_post_reader, php_default_treat_data, php_default_input_filter,
    php_call_shutdown_functions, zend_observer_fcall_end_all, php_handle_auth_data, php_handle_aborted_connection,
    SAPI_HEADER_SENT_SUCCESSFULLY, SAPI_HEADER_SEND_FAILED, TRACK_VARS_FILES, IS_UNDEF, IS_STRING, // php-src defines the E_CORE and E_FATAL_ERRORS groups.
    E_WARNING, E_CORE_WARNING, E_COMPILE_WARNING, E_USER_WARNING,
    E_NOTICE, E_USER_NOTICE, E_DEPRECATED, E_USER_DEPRECATED,
    E_CORE, E_FATAL_ERRORS,
}

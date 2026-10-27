#ifndef RAPIRA_HTTP_H
#define RAPIRA_HTTP_H

#include "rapira_sapi.h"

// rust glue
extern void rapira_rs_exchange_drop(void *job);
extern bool rapira_rs_http_server_name(zval *return_value);
extern int64_t rapira_rs_http_server_port(void);

// C data sits before zend_object std, as in rapira_sapi.h
typedef struct {
    void *unit;   // Rust-owned Box<ExchangeState>; NULL when released.
    zval request; // cached Rapira\Http\Request; IS_UNDEF until getRequest()
    zend_object std;
} rapira_exchange_obj;

// Class entries read by rapira_http.c. Rust declares its own externs.
extern zend_class_entry *rapira_ce_http_multipart;

// The register function of the HTTP plugin.
void rapira_http_register_classes(void);

static zend_always_inline rapira_exchange_obj *
rapira_exchange_from(zend_object *obj) {
    // std is embedded after the C fields.
    return (rapira_exchange_obj *)((char *)obj -
                                   offsetof(rapira_exchange_obj, std));
}

#endif // RAPIRA_HTTP_H

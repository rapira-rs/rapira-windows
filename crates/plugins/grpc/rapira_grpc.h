#ifndef RAPIRA_GRPC_H
#define RAPIRA_GRPC_H

#include "rapira_sapi.h"

// rust glue
extern void rapira_rs_grpc_drop(void *state);

// C data sits before zend_object std, as in rapira_sapi.h
typedef struct {
    void *state;   // Rust-owned Box<GrpcState>; NULL until receive().
    zval message;  // Cached message; IS_UNDEF until getMessage().
    zval context;  // Cached context; IS_UNDEF until getContext().
    zval metadata; // Cached metadata; IS_UNDEF until getResponseMetadata().
    zend_object std;
} rapira_grpc_call_obj;

typedef struct {
    void *state; // Borrowed from the call; its free_obj sets this to NULL.
    zend_object std;
} rapira_grpc_metadata_obj;

// Class entries read by rapira_grpc.c. Rust declares its own externs.
extern zend_class_entry *rapira_ce_grpc_status_code;
extern zend_class_entry *rapira_ce_grpc_method_kind;
extern zend_class_entry *rapira_ce_grpc_protocol;
extern zend_class_entry *rapira_ce_grpc_status;
extern zend_class_entry *rapira_ce_grpc_metadata;

// The register function of the gRPC plugin.
void rapira_grpc_register_classes(void);

static zend_always_inline rapira_grpc_call_obj *
rapira_grpc_call_from(zend_object *obj) {
    return (rapira_grpc_call_obj *)((char *)obj -
                                    offsetof(rapira_grpc_call_obj, std));
}

static zend_always_inline rapira_grpc_metadata_obj *
rapira_grpc_metadata_from(zend_object *obj) {
    return (
        rapira_grpc_metadata_obj *)((char *)obj -
                                    offsetof(rapira_grpc_metadata_obj, std));
}

#endif // RAPIRA_GRPC_H

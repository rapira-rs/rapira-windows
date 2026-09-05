#ifndef RAPIRA_CLASSES_H
#define RAPIRA_CLASSES_H

#include "wrapper.h"
#include "zend_API.h"
#include "zend_property_hooks.h"

// Rust implements these functions.
extern void rapira_rs_exchange_drop(void *job);
extern void rapira_rs_dispatcher_release(void);

// wrapper.h defines the class entry declarations and object layouts for Bindgen.

// Call this function from PHP_MINIT_FUNCTION.
void rapira_register_classes(void);

// Export the file-scoped ext_functions array through this function.
const zend_function_entry *rapira_php_functions(void);

static zend_always_inline void rapira_throw_or_backstop(const char *what) {
    if (!EG(exception)) {
        zend_throw_error(NULL, "%s failed", what);
    }
}

// Convert an embedded zend_object to its containing C object. https://www.zend.com/resources/php-extensions/embedding-c-data-into-php-objects
static zend_always_inline rapira_exchange_obj *
rapira_exchange_from(zend_object *obj) {
    // Subtract the std offset because the C fields occur before std.
    return (rapira_exchange_obj *)((char *)obj -
                                   XtOffsetOf(rapira_exchange_obj, std));
}

static zend_always_inline rapira_dispatcher_info_obj *
rapira_dispatcher_info_from(zend_object *obj) {
    return (rapira_dispatcher_info_obj *)((char *)obj -
                                          XtOffsetOf(rapira_dispatcher_info_obj,
                                                     std));
}

#endif // RAPIRA_CLASSES_H

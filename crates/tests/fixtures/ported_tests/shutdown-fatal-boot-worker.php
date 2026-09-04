<?php
// A fatal error in a bootstrap shutdown function must let the worker exit correctly.
register_shutdown_function(static function (): void {
    trigger_error('boot shutdown bomb', E_USER_ERROR); // php_call_shutdown_functions contains the error with zend_try.
});
$handler = static function (): void {
    echo 'ok';
};
while (\Rapira\handle_request($handler)) {
}

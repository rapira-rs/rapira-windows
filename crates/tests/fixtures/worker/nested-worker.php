<?php
// The nesting guard must reject the inner call before it receives a job or changes the active request.
$handler = static function (): void {
    try {
        \Rapira\handle_request(static function (): void {});
        echo 'inner-returned';
    } catch (\Error $e) {
        echo 'nested: ', $e->getMessage();
    }
};
while (\Rapira\handle_request($handler)) {
}

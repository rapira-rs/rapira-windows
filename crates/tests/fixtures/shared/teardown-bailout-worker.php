<?php
// The handler returns before teardown. An open output buffer callback causes a fatal error during php_output_end_all() in rapira_request_teardown(). The zend_try in that C function must contain the bailout.
ini_set('display_errors', '0'); // Write the fatal error to the log and exclude it from the response body.

class Counter
{
    public static int $n = 0;
}

$handler = static function (): void {
    Counter::$n++;
    if (($_GET['boom'] ?? '') === '1') {
        // The callback runs during teardown when php_output_end_all() removes the buffer.
        ob_start(static function (string $buf): string {
            trigger_error('boom during output flush', E_USER_ERROR); // E_USER_ERROR causes zend_bailout.
            return $buf;
        });
        echo "never flushes cleanly";            // The buffer does not send this output to ub_write.
        return;
    }
    header('Content-Type: text/plain');
    echo "ok counter=" . Counter::$n;
};

while (\Rapira\handle_request($handler)) {
    gc_collect_cycles();
}

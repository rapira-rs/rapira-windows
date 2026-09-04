<?php
// display_errors=Off excludes fatal error text from the stream. The recycle path returns status 500.
ini_set('display_errors', '0');
class Counter { public static int $n = 0; }
$handler = static function (): void {
    Counter::$n++;
    if (($_GET['boom'] ?? '') === '1') {
        ob_start(static function ($buf, $phase) {
            trigger_error('fatal in flush', E_USER_ERROR); // Causes a bailout from php_output_end_all().
        });
        echo 'buffered';
        \rapira_finish_request();   // The flush runs the output buffer handler, which causes a fatal error.
        echo 'resumed-after-fatal'; // The bailout ends the job before this statement.
        return;
    }
    header('Content-Type: text/plain');
    echo 'ok counter=' . Counter::$n;
};
while (\Rapira\handle_request($handler)) {
}

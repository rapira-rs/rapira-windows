<?php
class Counter { public static int $n = 0; }
$handler = static function (): void {
    Counter::$n++;
    if (($_GET['boom'] ?? '') === '1') {
        exit(1); // zend_bailout occurs before output and returns status 500. A string argument sends output first and returns status 200.
    }
    header('Content-Type: text/plain');
    echo "ok counter=" . Counter::$n;
};
while (\Rapira\handle_request($handler)) { gc_collect_cycles(); }

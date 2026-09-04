<?php
// exit() in a handler finishes the response. The worker loop and its state remain active.
$n = 0;
$handler = static function () use (&$n): void {
    $n++;
    echo 'n=', $n;
    if (($_GET['die'] ?? '') === '1') {
        exit;
    }
};
while (\Rapira\handle_request($handler)) {
}

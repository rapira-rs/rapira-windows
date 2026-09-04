<?php
// handle_request() must reject dispatcher mode before it changes the shared input. A subsequent request verifies that the input remains valid.

use Rapira\Exception\ClosedException;
use Rapira\Exception\NotInWorkerModeError;

try {
    \Rapira\handle_request(static function (): void {});
} catch (NotInWorkerModeError $e) {
    \Rapira\log('gate ' . $e::class);
}

try {
    rapira_finish_request();
} catch (\Error $e) {
    \Rapira\log('finish-gate');
}

$d = \Rapira\get_dispatcher();
try {
    while (true) {
        $ex = $d->receive();
        $ex->writeBody('ok');
    }
} catch (ClosedException) {
}

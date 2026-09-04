<?php
// This resident worker returns its process ID for each request. The value verifies process continuity.

use Rapira\Exception\ClosedException;

$d = \Rapira\get_dispatcher();
try {
    while (true) {
        $ex = $d->receive();
        $ex->writeHead(200, ['content-type' => ['text/plain']]);
        $ex->writeBody('ok:' . getmypid());
    }
} catch (ClosedException) {
}

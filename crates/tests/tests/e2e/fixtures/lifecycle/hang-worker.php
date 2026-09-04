<?php
// ?hang=1 blocks the request in a native sleep call. The request_terminate_timeout watchdog must stop the process.

use Rapira\Exception\ClosedException;

$d = \Rapira\get_dispatcher();
try {
    while (true) {
        $ex = $d->receive();
        parse_str(parse_url($ex->getRequest()->target, PHP_URL_QUERY) ?: '', $q);
        if (($q['hang'] ?? '') === '1') {
            while (true) {
                usleep(100000);
            }
        }
        $ex->writeHead(200, ['content-type' => ['text/plain']]);
        $ex->writeBody('ok:' . getmypid());
    }
} catch (ClosedException) {
}

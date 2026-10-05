<?php
use Rapira\Exception\ClosedException;

$d = \Rapira\get_dispatcher();
try {
    while (true) {
        $ex = $d->receive();
        if ($ex->getRequest()->target === '/hold') {
            set_time_limit(0);
            while (true) {
                usleep(100000);
            }
        }
        $ex->writeHead(200);
        $ex->writeBody('ok');
    }
} catch (ClosedException) {
}

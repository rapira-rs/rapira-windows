<?php
use Rapira\Exception\ClosedException;

sleep(3);
$d = \Rapira\get_dispatcher();
try {
    while (true) {
        $ex = $d->receive();
        $ex->writeHead(200);
        $ex->writeBody('ok');
    }
} catch (ClosedException) {
}

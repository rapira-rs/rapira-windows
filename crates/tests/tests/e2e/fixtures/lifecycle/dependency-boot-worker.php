<?php
use Rapira\Exception\ClosedException;

if (!file_exists(__DIR__ . '/up.flag')) {
    throw new RuntimeException('dependency down');
}
\Rapira\log('dependency ready');
$d = \Rapira\get_dispatcher();
try {
    while (true) {
        $ex = $d->receive();
        $ex->writeHead(200);
        $ex->writeBody('ok');
    }
} catch (ClosedException) {
}

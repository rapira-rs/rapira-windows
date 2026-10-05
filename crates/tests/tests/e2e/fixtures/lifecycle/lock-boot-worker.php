<?php
use Rapira\Exception\ClosedException;

$lock = fopen(__DIR__ . '/boot.lock', 'c');
if (!flock($lock, LOCK_EX | LOCK_NB)) {
    throw new RuntimeException('boot lock held');
}
\Rapira\log('boot lock acquired');
$d = \Rapira\get_dispatcher();
try {
    while (true) {
        $ex = $d->receive();
        $ex->writeHead(200);
        $ex->writeBody('ok');
    }
} catch (ClosedException) {
}

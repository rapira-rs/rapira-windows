<?php

use Rapira\Exception\ClosedException;
use Rapira\Exception\WorkDiscardedException;

$d = \Rapira\get_dispatcher();
$sequence = 0;
$result = null;
try {
    while (true) {
        $ex = $d->receive();
        ++$sequence;
        if ($ex->getRequest()->target === '/download') {
            // Winsock can accept one large buffer at once. Several writes leave data pending when the client resets.
            $size = 32 * 1024 * 1024;
            $ex->writeHead(200, ['content-length' => [(string) $size]]);
            try {
                $chunk = str_repeat('x', $size / 4);
                for ($i = 0; $i < 4; $i++) {
                    $ex->writeBody($chunk, eos: false);
                }
                $deadline = microtime(true) + 5;
                while (!$ex->isCancelled() && microtime(true) < $deadline) {
                    usleep(1_000);
                }
                $ex->writeBody('', eos: true);
                $result = 'timeout';
            } catch (WorkDiscardedException) {
                $result = 'discarded';
            }
            continue;
        }
        $ex->writeBody(getmypid() . ":{$sequence}:{$result}");
    }
} catch (ClosedException) {
}

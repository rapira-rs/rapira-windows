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
            // Windows accepts one send of any size while the socket send buffer is below its limit and refuses the next send, so several frames make the second socket write stall in hyper. https://learn.microsoft.com/en-us/troubleshoot/windows-server/networking/slow-performance-copy-data-tcp-server-sockets-api
            $chunk = str_repeat('x', 8 * 1024 * 1024);
            $chunks = 4;
            $ex->writeHead(200, ['content-length' => [(string) ($chunks * strlen($chunk))]]);
            try {
                for ($i = 0; $i < $chunks; ++$i) {
                    $ex->writeBody($chunk, eos: false);
                }
                // Shorter than the read timeout of the test, so a stuck test fails on the state assertion instead of a read timeout.
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

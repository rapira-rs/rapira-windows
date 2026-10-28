<?php

use Rapira\Exception\ClosedException;

set_time_limit(0);
$dispatcher = Rapira\get_dispatcher();
try {
    while (true) {
        $exchange = $dispatcher->receive();
        parse_str(parse_url($exchange->getRequest()->target, PHP_URL_QUERY) ?: '', $query);
        usleep(6_000_000);
        set_time_limit((int) $query['limit']);
        Rapira\log('stream timer reset');
        $chunk = str_repeat('x', 64 * 1024);
        for ($i = 0; $i < 512; ++$i) {
            $exchange->writeBody($chunk, eos: false);
        }
        usleep(3_000_000);
        $exchange->writeBody('done');
    }
} catch (ClosedException) {
}

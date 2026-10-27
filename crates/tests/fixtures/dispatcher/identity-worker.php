<?php

$dispatcher = \Rapira\get_dispatcher();
$info = $dispatcher->getInfo();
$identity = 'name=' . $info->serverName() . ' port=' . $info->serverPort();
\Rapira\log('identity', context: ['result' => $identity]);
try {
    while (true) {
        $dispatcher->receive()->writeBody($identity);
    }
} catch (\Rapira\Exception\ClosedException) {
}

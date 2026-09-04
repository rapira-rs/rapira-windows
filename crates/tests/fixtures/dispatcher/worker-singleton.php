<?php

$d = \Rapira\get_dispatcher();

try {
    clone $d;
    $clone = 'allowed';
} catch (\Error) {
    $clone = 'blocked';
}

// No response is available. Write the results to the application log.
\Rapira\log('dispatcher', context: [
    'class' => $d::class,
    'name' => $d->name(),
    'same' => $d === \Rapira\get_dispatcher(),
    'http' => $d instanceof \Rapira\Http\HttpDispatcher,
    'base' => $d instanceof \Rapira\Dispatcher,
    'clone' => $clone,
]);

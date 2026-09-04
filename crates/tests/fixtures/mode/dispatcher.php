<?php

$mode = \Rapira\get_mode();

// No response is available. Write the results to the application log.
// json_encode cannot encode a pure enum. The context contains only the name and comparison results.
\Rapira\log('mode', context: [
    'name' => $mode->name,
    'case' => $mode === \Rapira\Mode::Dispatcher,
    'class' => $mode::class,
]);

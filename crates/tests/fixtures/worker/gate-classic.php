<?php

try {
    \Rapira\handle_request(static function (): void {});
    echo "returned\n";
} catch (\Rapira\Exception\NotInWorkerModeError $e) {
    echo 'class: ', $e::class, "\n";
    echo 'rapira: ', $e instanceof \Rapira\Exception\RapiraThrowable ? 'yes' : 'no', "\n";
}

// ZPP validates arguments before it checks the mode. A non-callable argument causes a TypeError in all modes.
try {
    \Rapira\handle_request('nope');
} catch (\TypeError) {
    echo "type-error\n";
}
echo 'done';

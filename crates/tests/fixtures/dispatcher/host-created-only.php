<?php

try {
    new \Rapira\Internal\Http\Dispatcher();
    echo "constructed\n";
} catch (\Error $e) {
    // The engine rejects the private constructor before it runs the body.
    echo 'blocked: ', $e->getMessage(), "\n";
}
echo 'done';

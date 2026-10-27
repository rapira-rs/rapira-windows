<?php

class FatalOnRelease extends \RuntimeException
{
    public function __destruct()
    {
        trigger_error('fatal while the exception is released', E_USER_ERROR);
    }
}

$counter = 0;
while (\Rapira\handle_request(static function () use (&$counter): void {
    ++$counter;
    if (($_GET['boom'] ?? '') === 'shutdown') {
        register_shutdown_function(static function (): void {
            throw new FatalOnRelease('from a shutdown function');
        });
    }
    if (($_GET['boom'] ?? '') === 'teardown') {
        ob_start(static function (string $buffer): string {
            throw new FatalOnRelease('from an output callback');
        });
        echo 'buffered';
        return;
    }
    echo 'ok counter=' . $counter;
})) {
}

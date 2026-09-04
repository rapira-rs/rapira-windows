<?php
// handle_request() returns false when the host closes the input. The loop exits, and the script completes.
$served = 0;
$handler = static function () use (&$served): void {
    $served++;
    echo 'n=', $served;
};
while (\Rapira\handle_request($handler)) {
}
\Rapira\log('loop-exited served=' . $served);

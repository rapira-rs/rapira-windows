<?php
$handler = static function (): void {
    // Concurrent queued requests remain live long enough to occupy all four threads.
    usleep(20000);
    echo 'ok';
};
while (\Rapira\handle_request($handler)) {
}

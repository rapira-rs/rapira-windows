<?php
$handler = static function (): void {
    header('Content-Type: text/plain');
    echo getenv('PATH'), "\n";
};
require __DIR__ . '/nope-does-not-exist.php'; // Causes a fatal error because the required file does not exist.
while (\Rapira\handle_request($handler)) { gc_collect_cycles(); }

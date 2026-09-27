<?php

while (\Rapira\handle_request(static function (): void {
    if (!function_exists('opcache_is_script_cached')) {
        echo 'skip';
        return;
    }
    $path = getenv('RAPIRA_CACHE_FILE');
    if ($_SERVER['REQUEST_URI'] === '/compile') {
        opcache_compile_file($path);
    }
    echo opcache_is_script_cached($path) ? 'cached' : 'missing';
})) {}

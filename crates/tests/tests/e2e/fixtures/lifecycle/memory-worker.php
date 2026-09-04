<?php
$held = [];
$handler = static function () use (&$held): void {
    $held[] = str_repeat('m', 256 * 1024);
    echo json_encode([
        'held' => count($held),
        'memory' => memory_get_usage(true),
    ], JSON_THROW_ON_ERROR);
};
while (\Rapira\handle_request($handler)) {
}

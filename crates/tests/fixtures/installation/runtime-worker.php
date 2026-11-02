<?php
$handler = static function (): void {
    header('content-type: application/json');
    echo json_encode([
        'version' => PHP_VERSION,
        'zts' => (int) PHP_ZTS,
        'extensions' => array_map('strtolower', get_loaded_extensions()),
    ], JSON_THROW_ON_ERROR);
};
while (\Rapira\handle_request($handler)) {
}

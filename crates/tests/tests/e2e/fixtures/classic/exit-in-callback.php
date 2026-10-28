<?php

switch ($_GET['exit'] ?? '') {
    case 'handler':
        set_exception_handler(static function (Throwable $e): void {
            http_response_code(404);
            echo "Not Found\n";
            exit(1);
        });
        throw new RuntimeException('no route');
    case 'shutdown-fatal':
        ini_set('display_errors', '0');
        ini_set('log_errors', '0');
        echo "partial page\n";
        register_shutdown_function(static function (): void {
            trigger_error('shutdown failed', E_USER_ERROR);
        });
        break;
    case 'shutdown':
        echo "ok\n";
        register_shutdown_function(static function (): void {
            echo "bye\n";
            exit(1);
        });
        break;
}

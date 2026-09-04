<?php
$_ENV['boot_mark'] = 'set-at-boot';
$handler = static function (): void {
    // Compiling a new file that uses $_ENV triggers the reset.
    require __DIR__ . '/late-env.php';
    echo $_ENV['boot_mark'] ?? 'lost';
};
while (\Rapira\handle_request($handler)) {
}

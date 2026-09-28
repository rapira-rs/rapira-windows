<?php
// A later $_SERVER build must not add a reference to this null value.
$argv = null;
$boot = require __DIR__ . '/values.php';
$handler = static function () use ($boot): void {
    header('Content-Type: application/json');
    echo $boot;
};
while (\Rapira\handle_request($handler)) {
}

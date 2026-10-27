<?php

$handler = static function (): void {
    header('Content-Length: 11');
    echo 'hello world';
};
if (\Rapira\get_mode() === \Rapira\Mode::Worker) {
    while (\Rapira\handle_request($handler)) {
    }
} else {
    $handler();
}

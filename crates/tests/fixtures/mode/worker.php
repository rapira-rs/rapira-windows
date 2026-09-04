<?php
// Capture the mode once during boot and again for each job. The enum case must remain the same.
$boot = \Rapira\get_mode();
$handler = static function () use ($boot): void {
    $mode = \Rapira\get_mode();
    echo $mode->name,
        ':', $mode === \Rapira\Mode::Worker ? 'case' : 'copy',
        ':', $mode === $boot ? 'same' : 'new',
        ':', $mode instanceof \BackedEnum ? 'backed' : 'unbacked';
};
while (\Rapira\handle_request($handler)) {
}

<?php
// The callback runs when the application cycle ends.
header_register_callback(static function (): void {
    str_repeat('x', 1 << 40);
});
// Reuse the previous cycle's allocations before the dispatcher is requested.
$objects = [];
for ($i = 0; $i < 50; $i++) {
    $objects[] = new ArrayObject([$i]);
}
$ex = \Rapira\get_dispatcher()->receive();
$ex->writeBody((string) getmypid());

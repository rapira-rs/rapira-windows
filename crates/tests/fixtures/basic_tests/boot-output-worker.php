<?php
echo "WORKER-BOOT-OUTPUT";          // Send output to ub_write without a context before the loop.
while (ob_get_level() > 0) {
	ob_end_flush();                 // Send the buffered output to ub_write here.
}
$handler = static function (): void {
	header('Content-Type: text/plain');
	echo "served";
};
while (\Rapira\handle_request($handler)) {
}

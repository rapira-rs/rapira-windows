<?php
$handler = static function (): void {
	// Discard the result. The read creates the stream that the next line counts.
	file_get_contents('php://input');
	header('Content-Type: text/plain');
	echo "streams=", count(get_resources('stream'));
};
while (\Rapira\handle_request($handler)) {
	gc_collect_cycles();
}

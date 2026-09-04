<?php
class Track
{
	public static int $done = 0;
}

$handler = static function (): void {
	header('Content-Type: text/plain');
	if (($_GET['probe'] ?? '') === '1') {
		echo "done=", Track::$done, " aborted=", connection_aborted();
		return;
	}
	// The test drops the receiver after the 'held' event. The delay lets the receiver close before the first write.
	\Rapira\log('held');
	usleep(300000);
	echo "payload\n"; // The SAPI calls php_handle_aborted_connection after the aborted write.
	Track::$done++; // The client disconnect must stop execution before this statement.
};
while (\Rapira\handle_request($handler)) {
	gc_collect_cycles();
}

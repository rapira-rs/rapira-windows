<?php
class TrackIgnore
{
	public static int $reached = 0;
}
$handler = static function (): void {
	header('Content-Type: text/plain');
	if (($_GET['probe'] ?? '') === '1') {
		echo "reached=", TrackIgnore::$reached, " aborted=", connection_aborted();
		return;
	}
	ignore_user_abort(true);
	// The test drops the receiver after the 'held' event. The delay lets the receiver close before the write.
	\Rapira\log('held');
	usleep(300000);
	echo "payload\n"; // The aborted write must not cause a bailout.
	TrackIgnore::$reached++; // ignore_user_abort=1 lets the handler continue.
};
while (\Rapira\handle_request($handler)) {
	gc_collect_cycles();
}

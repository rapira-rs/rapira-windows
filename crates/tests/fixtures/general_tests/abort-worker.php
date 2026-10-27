<?php
class Track
{
	public static int $served = 0;
	public static int $done = 0;
}

function write_after_leave(): void
{
	// Give the client time to close the connection before this write.
	\Rapira\log('held');
	usleep(300000);
	echo "payload\n";
	Track::$done++;
}

final class WriteOnDestruct
{
	public function __destruct()
	{
		write_after_leave();
	}
}

$handler = static function (): void {
	Track::$served++;
	header('Content-Type: text/plain');
	switch ($_GET['case'] ?? '') {
		case 'probe':
			echo "served=", Track::$served, " done=", Track::$done, " aborted=", connection_aborted();
			return;
		case 'echo':
			write_after_leave();
			return;
		case 'flush':
			ob_start();
			\Rapira\log('held');
			usleep(300000);
			echo "payload\n";
			return;
		case 'shutdown':
			register_shutdown_function('write_after_leave');
			return;
		case 'destruct':
			$o = new WriteOnDestruct();
			return;
	}
};
while (\Rapira\handle_request($handler)) {
	gc_collect_cycles();
}

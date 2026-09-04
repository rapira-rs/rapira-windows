<?php
$served = 0;
$handler = static function () use (&$served): void {
	$served++;
	if ($served === 1) {
		$d = new class {
			public function __destruct()
			{
				throw new \RuntimeException('dtor boom');
			}
		};
		// The closure retains $d until PHP releases the shutdown table after the job. The destructor then throws outside the handler frame.
		register_shutdown_function(static function () use ($d): void {});
	}
	echo 'served=', $served;
};
while (\Rapira\handle_request($handler)) {
}

<?php
$handler = static function (): void {
	if (!extension_loaded('Zend OPcache')) {
		echo 'skip';
		return;
	}
	// PHP registers opcache_get_status() when accel_startup() fails. The function returns false in this case.
	$status = opcache_get_status(false);
	echo is_array($status) && ($status['opcache_enabled'] ?? false)
		? 'opcache:enabled'
		: 'opcache:disabled';
};
while (\Rapira\handle_request($handler)) {
}

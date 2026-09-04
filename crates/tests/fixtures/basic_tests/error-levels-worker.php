<?php
// The engine fills PG(last_error_*) for all masks. Only the teardown log can apply error_reporting(). Set the mask during bootstrap because worker mode restores INI settings for each cycle.
error_reporting(E_ALL & ~E_DEPRECATED & ~E_USER_DEPRECATED);

$handler = static function (): void {
	switch ($_GET['step'] ?? '') {
		case 'deprecated':
			trigger_error('MASKED-DEPRECATION', E_USER_DEPRECATED);
			break;
		case 'warn':
			trigger_error('REPORTED-WARNING', E_USER_WARNING);
			break;
		case 'boom':
			// An uncaught exception reaches php_error_cb as E_ERROR without a bailout. trigger_error(E_USER_ERROR) also reports a deprecation in PHP 8.4 and later.
			throw new \RuntimeException('REPORTED-FATAL');
		case 'silent-fatal':
			error_reporting(0);
			throw new \RuntimeException('SILENCED-FATAL');
		case 'logged':
			// The error is not masked. The SAPI log callback also reports it when log_errors is enabled.
			error_reporting(E_ALL);
			ini_set('log_errors', '1');
			trigger_error('LOGGED-DEPRECATION', E_USER_DEPRECATED);
			break;
	}
	echo 'ok';
};
while (\Rapira\handle_request($handler)) {
}

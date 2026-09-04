<?php
$handler = static function (): void {
	header('Content-Type: text/plain');
	echo 'ok';
};
while (\Rapira\handle_request($handler)) {
}
trigger_error('worker exiting', E_USER_WARNING); // Stores the warning in PG(last_error_message) after the loop.

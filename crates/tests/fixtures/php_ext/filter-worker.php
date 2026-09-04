<?php
$handler = static function (): void {
	if (!extension_loaded('filter')) {
		echo 'skip';
		return;
	}
	if (($_GET['boom'] ?? '') === '1') {
		ini_set('display_errors', '0');
		// An unknown filter ID causes a warning. A non-integer filter causes an exception.
		filter_var('x', []);
		return;
	}
	echo 'filter:' . filter_var('a@b.com', FILTER_VALIDATE_EMAIL);
};
while (\Rapira\handle_request($handler)) {
}

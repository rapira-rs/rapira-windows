<?php
$handler = static function (): void {
	if (!extension_loaded('fileinfo')) {
		echo 'skip';
		return;
	}
	if (($_GET['boom'] ?? '') === '1') {
		ini_set('display_errors', '0');
		// The object constructor throws an exception when the magic database is missing. finfo_open() causes a warning.
		new finfo(FILEINFO_NONE, '/nonexistent-dir-xyz/magic.mgc');
		return;
	}
	echo 'finfo:' . (new finfo(FILEINFO_MIME_TYPE))->buffer('hello');
};
while (\Rapira\handle_request($handler)) {
}

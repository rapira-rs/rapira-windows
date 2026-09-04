<?php
// This resident worker handles each request that an extension sends through `exec`.
$handler = static function (): void {
	header('Content-Type: text/plain');
	echo 'ok:' . ($_GET['from'] ?? '?');
};
while (\Rapira\handle_request($handler)) {
}

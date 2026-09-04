<?php
$handler = static function (): void {
	ini_set('display_errors', '0'); // Exclude error text from the body so that status 500 is the first output.
	throw new \RuntimeException('quiet boom');
};
while (\Rapira\handle_request($handler)) {
	gc_collect_cycles();
}

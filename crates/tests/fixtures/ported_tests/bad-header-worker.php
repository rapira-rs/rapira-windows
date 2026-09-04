<?php
$handler = static function (): void {
	// HTTP field names cannot contain a space, and field values cannot contain 0x01. sapi_header_op rejects only CR, LF, and NUL, so the SAPI must reject these fields.
	header('Content Type: text/html');
	header("X-Ctl: \x01");
	header('X-Keep: kept');
	http_response_code(201);
	echo 'body';
};
while (\Rapira\handle_request($handler)) {
	gc_collect_cycles();
}

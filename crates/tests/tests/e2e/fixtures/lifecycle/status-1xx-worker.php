<?php
// http_response_code(103) sets the final response status to 103. The HTTP server changes it because hyper changes a final 1xx status to 500 and closes the connection with an error. https://github.com/hyperium/hyper/blob/6371cd425017155f7fbecef0e57b218edbe6a93a/src/proto/h1/role.rs#L392-L408
$handler = static function (): void {
	http_response_code(103);
	echo "body";
};
while (\Rapira\handle_request($handler)) {
}

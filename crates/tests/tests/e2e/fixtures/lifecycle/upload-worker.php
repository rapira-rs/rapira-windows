<?php
// Report the parsed upload. The response contains 'NO FILE' if the multipart boundary is invalid.
$handler = static function (): void {
    $f = $_FILES['file'] ?? null;
    if ($f === null) {
        echo 'NO FILE';
        return;
    }
    echo $f['name'], '|', $f['error'], '|', file_get_contents($f['tmp_name']), '|', $f['tmp_name'];
};
while (\Rapira\handle_request($handler)) {
}

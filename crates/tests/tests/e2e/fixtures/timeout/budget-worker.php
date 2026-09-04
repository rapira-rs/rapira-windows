<?php
$handler = static function (): void {
    if (isset($_GET['spin'])) {
        while (true) {
        }
    }
    echo 'ok';
};
while (\Rapira\handle_request($handler)) {
}

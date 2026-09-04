<?php
$handler = static function (): void {
    if (isset($_GET['arm'])) {
        set_time_limit(1);
        echo 'armed';
        return;
    }
    if (isset($_GET['work'])) {
        $until = hrtime(true) + 1500000000;
        while (hrtime(true) < $until) {
        }
    }
    echo 'fresh';
};
while (\Rapira\handle_request($handler)) {
}

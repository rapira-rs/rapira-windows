<?php
class Seen
{
    public static int $runs = 0;
}
$handler = static function (): void {
    if (($_GET['probe'] ?? '') === 'count') {
        echo 'runs=', Seen::$runs;
        return;
    }
    Seen::$runs++;
    \Rapira\log('held');
    if (isset($_GET['release'])) {
        while (!file_exists($_GET['release'])) {
            usleep(10000);
            clearstatcache();
        }
    } else {
        usleep(400000);
    }
    echo 'done';
};
while (\Rapira\handle_request($handler)) {
}

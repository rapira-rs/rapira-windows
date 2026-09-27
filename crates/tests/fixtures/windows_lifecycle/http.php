<?php

\Rapira\log('lifecycle-bootstrap');
while (\Rapira\handle_request(static function (): void {
    if (isset($_GET['join'])) {
        header('Content-Length: 4');
        echo 'done';
        rapira_finish_request();
        \Rapira\log('lifecycle-join-blocked');
        sleep(30);
        return;
    }
    if (isset($_GET['hold'])) {
        \Rapira\log('lifecycle-hold-entered');
        sleep(30);
    }
    if (isset($_GET['drain'])) {
        \Rapira\log('lifecycle-drain-entered');
        $release = __DIR__ . '/release-drain';
        while (!is_file($release)) {
            clearstatcache(true, $release);
            usleep(10_000);
        }
        header('Content-Length: 8');
        echo 'finished';
        return;
    }
    header('Content-Length: 5');
    echo 'ready';
})) {
}

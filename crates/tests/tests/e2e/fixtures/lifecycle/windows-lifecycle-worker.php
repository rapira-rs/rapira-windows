<?php
\Rapira\log('lifecycle-php-bootstrap');

$handler = static function (): void {
    if (isset($_GET['join'])) {
        header('Content-Length: 4');
        echo 'done';
        rapira_finish_request();
        \Rapira\log('lifecycle-join-blocked');
        // The client disconnects and the HTTP layer finishes its drain while the PHP thread remains active.
        sleep(30);
        return;
    }

    if (isset($_GET['hold'])) {
        \Rapira\log('lifecycle-hold-entered');
        // The native sleep call must remain active after the extension drain and PHP thread join timeout.
        sleep(30);
    }

    if (isset($_GET['drain'])) {
        \Rapira\log('lifecycle-drain-entered');
        $release = __DIR__ . '/release-drain';
        while (true) {
            clearstatcache(true, $release);
            if (is_file($release)) {
                break;
            }
            usleep(10000);
        }
        header('Content-Length: 8');
        echo 'finished';
        return;
    }

    header('Content-Length: 5');
    echo 'ready';
};

while (\Rapira\handle_request($handler)) {
}

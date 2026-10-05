<?php
$flag = __DIR__ . '/served.flag';
if (file_exists($flag)) {
    \Rapira\log('reboot failed');
    return;
}
$ex = \Rapira\get_dispatcher()->receive();
$ex->writeHead(200);
$ex->writeBody('ok');
touch($flag);

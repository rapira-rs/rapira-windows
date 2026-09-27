<?php

$token = bin2hex(random_bytes(8));
$identity = trim(file_get_contents('identity.txt'));
$directory = __DIR__ . '/http-' . $token;
mkdir($directory);
file_put_contents($directory . '/identity.txt', $identity);
chdir($directory);

$reply = static function (string $input) use ($token): string {
    return json_encode([
        'plugin' => trim(file_get_contents('identity.txt')),
        'mode' => \Rapira\get_mode()->name,
        'token' => $token,
        'directory' => basename(getcwd()),
        'script' => basename($_SERVER['SCRIPT_FILENAME'] ?? __FILE__),
        'input' => $input,
    ]);
};

if (\Rapira\get_mode() === \Rapira\Mode::Worker) {
    while (\Rapira\handle_request(static function () use ($reply): void {
        echo $reply($_SERVER['REQUEST_URI']);
    })) {
    }
} else {
    $dispatcher = \Rapira\get_dispatcher();
    assert($dispatcher instanceof \Rapira\Http\HttpDispatcher);
    try {
        while (true) {
            $exchange = $dispatcher->receive();
            $exchange->writeBody($reply($exchange->getRequest()->target));
        }
    } catch (\Rapira\Exception\ClosedException) {
    }
}

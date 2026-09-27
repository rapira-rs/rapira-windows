<?php

$token = bin2hex(random_bytes(8));
$identity = trim(file_get_contents('identity.txt'));
$directory = __DIR__ . '/grpc-' . $token;
mkdir($directory);
file_put_contents($directory . '/identity.txt', $identity);
chdir($directory);
$dispatcher = \Rapira\get_dispatcher();
assert($dispatcher instanceof \Rapira\Grpc\GrpcDispatcher);
try {
    while (true) {
        $call = $dispatcher->receive();
        $call->respond(json_encode([
            'plugin' => trim(file_get_contents('identity.txt')),
            'mode' => \Rapira\get_mode()->name,
            'token' => $token,
            'directory' => basename(getcwd()),
            'script' => basename(__FILE__),
            'input' => $call->getMessage(),
        ]));
    }
} catch (\Rapira\Exception\ClosedException) {
}

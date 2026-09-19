<?php

set_time_limit(1);

$dispatcher = Rapira\get_dispatcher();
try {
    $work = $dispatcher->receive();
} catch (Rapira\Exception\ClosedException) {
    return;
}
if ($work instanceof Rapira\Grpc\UnaryCall) {
    $action = $work->getMessage();
    $work->respond('ready');
} else {
    $action = ltrim($work->getRequest()->target, '/');
    $work->writeBody('ready');
}
register_shutdown_function(function () {
    Rapira\log('receive-timer-finished');
});
try {
    $result = match ($action) {
        'poll' => $dispatcher->tryReceive(),
        'zero' => $dispatcher->receive(0),
        'finite' => $dispatcher->receive(1000),
        'closed' => $dispatcher->receive(),
    };
    $outcome = $result === null ? 'empty' : 'work';
} catch (Rapira\Exception\TimeoutException) {
    $outcome = 'timeout';
} catch (Rapira\Exception\ClosedException) {
    $outcome = 'closed';
}
Rapira\log('receive-timer-outcome', context: ['outcome' => $outcome]);
$end = hrtime(true) + 2000000000;
while (hrtime(true) < $end) {}
Rapira\log('receive-timer-survived');

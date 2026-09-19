<?php

$dispatcher = Rapira\get_dispatcher();
try {
    while (true) {
        $call = $dispatcher->receive();
        $call->respond(Rapira\get_mode()->name . ':' . $dispatcher->name() . ':' . getmypid());
    }
} catch (Rapira\Exception\ClosedException) {
}

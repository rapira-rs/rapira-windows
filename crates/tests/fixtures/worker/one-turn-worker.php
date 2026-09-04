<?php
// The script ends while the channel remains open. The cycle must return Recycle and start the worker again.
\Rapira\handle_request(static function (): void {
    echo 'once';
});
\Rapira\log('one-turn-done');

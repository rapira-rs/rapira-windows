<?php

// PSR-3 requires the `exception` context key. The chained exception verifies that the record contains more than the outermost frame. See https://www.php-fig.org/psr/psr-3/
try {
    try {
        throw new \RuntimeException('inner cause', 7);
    } catch (\RuntimeException $prev) {
        throw new \LogicException('outer failure', 42, $prev);
    }
} catch (\LogicException $e) {
    \Rapira\log('boom', \Rapira\LogLevel::Error, ['exception' => $e, 'order' => 'A-1']);
}

echo 'logged';

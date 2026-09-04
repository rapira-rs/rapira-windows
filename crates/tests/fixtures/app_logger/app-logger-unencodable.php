<?php

// json_encode cannot represent these values. log() must preserve the record and continue to the echo without an exception.
$fh = fopen('php://memory', 'rb');

\Rapira\log('hostile', \Rapira\LogLevel::Error, [
    'keep' => 'visible',
    'closure' => static fn (): int => 1,
    'resource' => $fh,
    'nan' => NAN,
    'inf' => INF,
    // These input bytes contain invalid UTF-8, which commonly causes json_encode to fail.
    'bytes' => "\xC3\x28\xA0\xA1",
    // A pure enum does not implement JsonSerializable and has no backing value.
    'pure_enum' => \Rapira\LogLevel::Debug,
]);

fclose($fh);
echo 'logged';

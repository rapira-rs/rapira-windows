<?php

// Monolog uses the class name as a wrapper key. It converts __toString to its return value and a resource to a marker. https://github.com/Seldaek/monolog/blob/147f303310f06334f03f409e49d7ad1e275ff05a/tests/Monolog/Formatter/NormalizerFormatterTest.php#L21-L58
class PlainNorm
{
    public $foo = 'fooValue';
}

class StringableNorm
{
    public function __toString(): string
    {
        return 'bar';
    }
}

// Monolog contains an exception from __toString in the logger. https://github.com/Seldaek/monolog/blob/147f303310f06334f03f409e49d7ad1e275ff05a/tests/Monolog/Formatter/NormalizerFormatterTest.php#L144-L165
class ToStringError
{
    public function __toString(): string
    {
        throw new \RuntimeException('Could not convert to string');
    }
}

$fh = fopen('php://memory', 'rb');

\Rapira\log('objects', \Rapira\LogLevel::Error, [
    'plain' => new PlainNorm(),
    'stringable' => new StringableNorm(),
    'boom' => new ToStringError(),
    'res' => $fh,
    'keep' => 'visible',
]);

fclose($fh);
echo 'logged';

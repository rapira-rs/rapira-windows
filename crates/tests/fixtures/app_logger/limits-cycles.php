<?php

// Monolog tests require object and array cycles to stop without a diagnostic. This test also requires adjacent values to remain. https://github.com/Seldaek/monolog/blob/147f303310f06334f03f409e49d7ad1e275ff05a/tests/Monolog/Formatter/NormalizerFormatterTest.php#L197-L235
$foo = new \stdClass();
$bar = new \stdClass();
$foo->bar = $bar;
$bar->foo = $foo;

$x = ['foo' => 'bar'];
$y = ['x' => &$x];
$x['y'] = &$y;

\Rapira\log('cycles', \Rapira\LogLevel::Error, [
    'objects' => $foo,
    'arrays' => $y,
    'keep' => 'visible',
]);

echo 'logged';

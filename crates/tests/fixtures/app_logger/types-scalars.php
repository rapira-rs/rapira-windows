<?php

// Monolog converts INF to "INF", -INF to "-INF", and NAN to "NaN". It replaces invalid bytes with U+FFFD and preserves the value. https://github.com/Seldaek/monolog/blob/147f303310f06334f03f409e49d7ad1e275ff05a/tests/Monolog/Formatter/NormalizerFormatterTest.php#L21-L58
\Rapira\log('scalars', \Rapira\LogLevel::Error, [
    'inf' => INF,
    'ninf' => -INF,
    'nan' => acos(4),
    // Monolog uses "\xB1\x31" to test an invalid leading byte followed by "1". https://github.com/Seldaek/monolog/blob/147f303310f06334f03f409e49d7ad1e275ff05a/tests/Monolog/Formatter/NormalizerFormatterTest.php#L295-L304
    'bad_utf8' => "\xB1\x31",
    'keep' => 'visible',
]);

echo 'logged';

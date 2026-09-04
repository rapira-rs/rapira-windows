<?php

// Monolog NormalizerFormatter limits the item count and depth but does not limit string length. This 5 MiB scalar produces a 5.24 MB record. https://github.com/Seldaek/monolog/blob/147f303310f06334f03f409e49d7ad1e275ff05a/src/Monolog/Formatter/NormalizerFormatter.php#L194-L226
\Rapira\log('huge-string', \Rapira\LogLevel::Error, [
    'blob' => str_repeat('A', 5 * 1024 * 1024),
    'keep' => 'visible',
]);

echo 'logged';

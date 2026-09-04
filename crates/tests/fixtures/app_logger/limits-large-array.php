<?php

// Monolog preserves an array at the 1,000-item limit and marks an array above the limit. https://github.com/Seldaek/monolog/blob/147f303310f06334f03f409e49d7ad1e275ff05a/tests/Monolog/Formatter/NormalizerFormatterTest.php#L263-L292
\Rapira\log('exactly-1000', \Rapira\LogLevel::Error, ['rows' => range(1, 1000)]);
\Rapira\log('over-cap', \Rapira\LogLevel::Error, ['rows' => range(1, 2000), 'keep' => 'visible']);

echo 'logged';

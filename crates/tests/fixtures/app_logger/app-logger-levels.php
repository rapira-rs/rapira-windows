<?php

// The call without a level verifies that C applies the stub default `LogLevel::Info`. The default is reflection metadata.
\Rapira\log('lvl-error', \Rapira\LogLevel::Error);
\Rapira\log('lvl-warning', \Rapira\LogLevel::Warning);
\Rapira\log('lvl-info', \Rapira\LogLevel::Info);
\Rapira\log('lvl-debug', \Rapira\LogLevel::Debug);
\Rapira\log('lvl-trace', \Rapira\LogLevel::Trace);
\Rapira\log('lvl-omitted');

echo 'logged';

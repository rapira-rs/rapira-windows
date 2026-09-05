<?php

\Rapira\log('exactly-1000', \Rapira\LogLevel::Error, ['rows' => range(1, 1000)]);

echo 'logged';

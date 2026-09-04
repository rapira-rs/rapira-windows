<?php

// The 20,000 nested arrays exceed the Zend stack limit. The JSON encoder writes null without a limit marker. https://github.com/php/php-src/blob/dab13a022a54f8bc03302f93ccb6484907ec1245/ext/json/json_encoder.c#L35-L41
$deep = 'bottom';
for ($i = 0; $i < 20000; $i++) {
    $deep = ['n' => $deep];
}

\Rapira\log('deep', \Rapira\LogLevel::Error, ['tree' => $deep, 'keep' => 'visible']);

echo 'logged';

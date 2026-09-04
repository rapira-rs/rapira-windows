<?php

// The precision INI directive has a default value of 14. The response body shows if the test php.ini overrides it. https://github.com/php/php-src/blob/dab13a022a54f8bc03302f93ccb6484907ec1245/main/main.c#L868

$d = \Rapira\get_dispatcher();
try {
    while (true) {
        $ex = $d->receive();
        $ex->writeBody('precision=' . ini_get('precision'));
    }
} catch (\Rapira\Exception\ClosedException) {
}

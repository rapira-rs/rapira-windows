<?php
// The server runs this front controller once for each request.
header('content-type: text/plain');
echo 'classic: ', $_SERVER['REQUEST_METHOD'], ' ', $_SERVER['REQUEST_URI'], "\n";

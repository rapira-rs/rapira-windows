<?php
ini_set('display_errors', '0');
session_start();            // Add the Set-Cookie header for PHPSESSID.
http_response_code(404);
throw new \RuntimeException('boom');

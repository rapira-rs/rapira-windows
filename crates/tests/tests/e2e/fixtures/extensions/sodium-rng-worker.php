<?php

use Rapira\Exception\ClosedException;

$id = bin2hex(random_bytes(8));
$body = extension_loaded('sodium')
	? 'pid=' . $id . ' first=' . bin2hex(sodium_crypto_shorthash_keygen())
	: 'skip';
$d = \Rapira\get_dispatcher();
try {
	while (true) {
		$ex = $d->receive();
		$ex->writeBody($body);
	}
} catch (ClosedException) {
}

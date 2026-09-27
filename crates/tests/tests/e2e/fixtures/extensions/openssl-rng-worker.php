<?php
// A bootstrap token distinguishes interpreters in the same process.
$id = bin2hex(random_bytes(8));

use Rapira\Exception\ClosedException;

// no skip guard: openssl is in every build, and a missing extension must fail the boot loudly
$first = bin2hex(openssl_random_pseudo_bytes(16));
$d = \Rapira\get_dispatcher();
try {
	while (true) {
		$ex = $d->receive();
		$ex->writeBody('pid=' . $id . ' first=' . $first);
	}
} catch (ClosedException) {
}

<?php
$handler = static function (): void {
	if (!extension_loaded('sodium')) {
		echo 'skip';
		return;
	}
	if (($_GET['boom'] ?? '') === '1') {
		ini_set('display_errors', '0');
		sodium_crypto_generichash('rapira', '', 1);
		return;
	}
	echo 'sodium:' . bin2hex(sodium_crypto_generichash('rapira'));
};
while (\Rapira\handle_request($handler)) {
}

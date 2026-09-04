<?php
function helper(string $big): void
{
	trigger_error('boom', E_USER_ERROR); // EG(last_fatal_error_backtrace) contains a reference to $big.
}
$handler = static function (): void {
	set_error_handler(static fn(): bool => true); // Handle the error so execution continues and the helper frame returns.
	if (($_GET['step'] ?? '') === 'boom') {
		helper(str_repeat('x', 20 * 1024 * 1024)); // After helper() returns, only the backtrace retains this 20 MB value.
		echo 'boomed';
		return;
	}
	echo 'mem=' . memory_get_usage(false);
};
while (\Rapira\handle_request($handler)) {
}

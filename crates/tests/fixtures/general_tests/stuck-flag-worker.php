<?php
class BailWrapper
{
	public $context;
	public function stream_open($p, $m, $o, &$op): bool
	{
		trigger_error('boom', E_USER_ERROR); // Causes a bailout without a memory limit dependency.
		return false;
	}
}
stream_wrapper_register('bail', BailWrapper::class); // Register a local wrapper with is_url=0.
while (\Rapira\handle_request(static function (): void {
	if (($_GET['step'] ?? '') === 'boom') {
		include 'bail://x'; // Set PG(in_user_include)=1, then cause a bailout before userspace.c restores it.
		return;
	}
	// data:// has is_url=1. PHP rejects it only if in_user_include remains set because allow_url_include is disabled.
	echo file_get_contents('data://text/plain,ok') === 'ok' ? 'PROBE_OK' : 'PROBE_REJECTED';
}));

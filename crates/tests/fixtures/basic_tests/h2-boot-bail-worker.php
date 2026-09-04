<?php
// Bootstrap opens a session whose save handler causes a fatal error on the first write. The first rapira_request_teardown() call causes a bailout when it flushes the session. The persistent counter uses getmypid() as its key so the in-process test can remove it. The served request reports cycle 2 after the worker recycles and starts again.
$dir = sys_get_temp_dir();
$sentinel = $dir . '/rapira_h2_sentinel_' . getmypid();
$boot = $dir . '/rapira_h2_boot_' . getmypid();

$n = (file_exists($boot) ? (int) file_get_contents($boot) : 0) + 1;
file_put_contents($boot, (string) $n);

class BoomOnce extends SessionHandler {
    public string $sentinel = '';
    public function write(string $id, string $data): bool {
        if (!file_exists($this->sentinel)) {
            @touch($this->sentinel);
            trigger_error('boot bail', E_USER_ERROR); // Causes a bailout in rapira_reset_session().
        }
        return true;
    }
}
$save = new BoomOnce();
$save->sentinel = $sentinel;
session_set_save_handler($save);
session_start();
$_SESSION['k'] = 'v'; // Modify the session so the flush writes it.

$handler = static function () use ($boot): void {
    echo (int) file_get_contents($boot);
};
while (\Rapira\handle_request($handler)) {
}

<?php
// A reference-counted object in a bootstrap global remains in the symbol table between jobs. Its destructor runs once at the end of the cycle.
class Kernel
{
    public int $calls = 0;

    public function tick(): int
    {
        return ++$this->calls;
    }

    public function __destruct()
    {
        \Rapira\log('boot-kernel destructed');
    }
}

$kernel = new Kernel();

$handler = static function (): void {
    if (!isset($GLOBALS['kernel'])) {
        echo 'kernel=gone';
        return;
    }
    echo 'kernel=ok calls=', $GLOBALS['kernel']->tick();
};
while (\Rapira\handle_request($handler)) {
}

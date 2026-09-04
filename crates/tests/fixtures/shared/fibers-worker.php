<?php

function fib(int $n): int
{
	return $n < 2 ? $n : fib($n - 1) + fib($n - 2);
}

function nest(int $depth): int
{
	if ($depth === 0) {
		return fib(12);
	}
	$inner = new Fiber(fn(): int => nest($depth - 1));
	$inner->start();                      // The inner fiber runs to completion without suspension.
	return $inner->getReturn();
}

$handler = static function (): void {
	$sum = 0;

	// Start 300 independent fibers and suspend each fiber two times. Each start or resume crosses the fiber and worker stack boundary.
	for ($i = 0; $i < 300; $i++) {
		$f = new Fiber(function (): int {
			$a = fib(14);                 // Recursion runs on the fiber stack.
			$b = Fiber::suspend($a);
			$c = Fiber::suspend($b + 1);
			return $a + $c;
		});
		$r1 = $f->start();
		$r2 = $f->resume($r1);
		$f->resume($r2);
		$sum += $f->getReturn();
	}
	// The sum is 300 * 755 = 226500.

	// Keep 25 fiber stacks active at the same time. The nested result adds 144.
	$sum += nest(25);

	header('Content-Type: text/plain');
	echo "fibers ok sum=$sum\n";
};

while (\Rapira\handle_request($handler)) {
	gc_collect_cycles();
}

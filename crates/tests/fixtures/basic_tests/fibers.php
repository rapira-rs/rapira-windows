<?php
function fib(int $n): int
{
    return $n < 2 ? $n : fib($n - 1) + fib($n - 2);
}

// Start 300 independent fibers and suspend each fiber two times. Each start or resume crosses the fiber and worker stack boundary. PHP changes EG(stack_base) and EG(stack_limit) at this boundary.
$sum = 0;
for ($i = 0; $i < 300; $i++) {
    $f = new Fiber(function (): int {
        $a = fib(14);                 // Recursion on the fiber stack returns 377.
        $b = Fiber::suspend($a);      // Control returns to the worker stack. The fiber resumes with 377.
        $c = Fiber::suspend($b + 1);  // Control returns to the worker stack. The fiber resumes with 378.
        return $a + $c;               // Returns 755.
    });
    $r1 = $f->start();                // The first suspension returns 377.
    $r2 = $f->resume($r1);            // The second suspension returns 378.
    $f->resume($r2);
    $sum += $f->getReturn();          // Add 755 for each fiber.
}
// The sum is 300 * 755 = 226500.

// Keep 25 nested fibers active. PHP saves the stack limits 25 times and restores them 25 times.
function nest(int $depth): int
{
    if ($depth === 0) {
        return fib(12);               // Returns 144.
    }
    $inner = new Fiber(fn(): int => nest($depth - 1));
    $inner->start();                  // The inner fiber runs to completion without suspension.
    return $inner->getReturn();
}
$sum += nest(25);                     // Add 144.

// A stale stack base from a fiber boundary causes this final compilation or echo to fail.
echo "fibers ok sum=$sum\n";          // The final sum is 226644.

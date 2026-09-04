<?php

// Select each test with the request target because dispatcher mode has no superglobals.

use Rapira\Exception\AlreadyFinalizedError;
use Rapira\Http\Exception\HeadAlreadyWrittenError;

$d = \Rapira\get_dispatcher();
try {
    while (true) {
        $ex = $d->receive();
        $req = $ex->getRequest();
        parse_str(parse_url($req->target, PHP_URL_QUERY) ?: '', $q);
        $probe = $q['probe'] ?? '';
        $out = [];

        if ($probe === 'double-finalize') {
            $ex->writeBody('first');
            try {
                $ex->writeBody('second');
            } catch (AlreadyFinalizedError $e) {
                \Rapira\log('double-finalize', context: ['class' => $e::class]);
            }
            continue;
        }
        if ($probe === 'double-head') {
            $ex->writeHead(201);
            try {
                $ex->writeHead(202);
            } catch (HeadAlreadyWrittenError $e) {
                $out[] = 'double-head:' . $e::class;
            }
            $ex->writeBody(implode(';', $out));
            continue;
        }
        if ($probe === 'busy') {
            try {
                $d->receive(0);
            } catch (\Error $e) {
                $out[] = 'busy:' . $e->getMessage();
            }
            $ex->writeBody(implode(';', $out));
            continue;
        }
        if ($probe === 'value-errors') {
            foreach ([99, 600] as $code) {
                try {
                    $ex->writeHead($code);
                } catch (\ValueError) {
                    $out[] = "range:$code";
                }
            }
            try {
                $ex->writeHead(200, ["bad name" => ['v']]);
            } catch (\ValueError) {
                $out[] = 'name';
            }
            try {
                $ex->writeHead(200, ['x-bad' => ["split\r\nx: y"]]);
            } catch (\ValueError) {
                $out[] = 'value';
            }
            try {
                $ex->writeHead(200, ['x-flat' => 'not-a-list']);
            } catch (\ValueError) {
                $out[] = 'shape';
            }
            try {
                $ex->writeHead(200, [123 => ['v']]);
            } catch (\ValueError) {
                $out[] = 'intkey';
            }
            try {
                $ex->writeHead(200, ['x-a' => [123]]);
            } catch (\ValueError) {
                $out[] = 'item';
            }
            $ex->writeBody(implode(';', $out));
            continue;
        }
        if ($probe === 'empty-chunk') {
            $ex->writeBody('', eos: false); // An empty body does not commit the response headers.
            $ex->writeHead(404);
            $ex->writeBody('body');
            continue;
        }
        if ($probe === 'verb-edges') {
            try {
                $d->tryReceive();
            } catch (\Error) {
                $out[] = 'try-busy';
            }
            try {
                $d->receive(-2);
            } catch (\ValueError) {
                $out[] = 'neg-timeout';
            }
            try {
                $ex->writeBody(implode(';', $out), eos: true);
                $ex->writeHead(500);
            } catch (HeadAlreadyWrittenError $e) {
                \Rapira\log('head-after-eos', context: ['class' => $e::class]);
            }
            continue;
        }
        if ($probe === 'interim') {
            $ex->writeHead(103, ['link' => ['</app.css>; rel=preload']]);
            $ex->writeHead(200, ['content-type' => ['text/plain']]);
            $ex->writeBody('after-interim finalized=' . var_export($ex->isFinalized(), true));
            continue;
        }
        if ($probe === 'chunks') {
            $ex->writeBody('one-', eos: false);
            $ex->writeBody('mid=' . var_export($ex->isFinalized(), true), eos: false);
            $ex->writeBody('', eos: true);
            continue;
        }
        if ($probe === 'head-after-chunk') {
            $ex->writeBody('partial', eos: false);
            try {
                $ex->writeHead(500);
            } catch (HeadAlreadyWrittenError) {
                $ex->writeBody('|locked', eos: true);
            }
            continue;
        }
        if ($probe === 'upgrade') {
            $ex->writeHead(101, ['upgrade' => ['example/1']]);
            try {
                $ex->writeHead(200);
            } catch (HeadAlreadyWrittenError $e) {
                \Rapira\log('101-locked', context: ['class' => $e::class]);
            }
            // Accept and discard this data because a 1xx response has no body.
            $ex->writeBody('dropped');
            continue;
        }
        if ($probe === 'multi') {
            $ref = ['r1'];
            $inner = 'c1';
            $h = ['x-multi' => ['a', 'b'], 'x-vref' => [&$inner]];
            $h['x-ref'] = &$ref;
            $ex->writeHead(200, $h);
            $ex->writeBody('done');
            continue;
        }
        if ($probe === 'info') {
            $info = $d->getInfo();
            $ex->writeBody(sprintf('pending=%d active=%d', $info->pendingCount(), $info->activeCount()));
            continue;
        }
        if ($probe === 'abandon') {
            unset($ex); // The host must fail this unfinalized unit and continue to serve requests.
            continue;
        }
        if ($probe === 'abandon-mid') {
            $ex->writeHead(200);
            $ex->writeBody('partial', eos: false);
            unset($ex); // The response headers are already sent. The host can only truncate the response.
            continue;
        }
        if ($probe === 'bail-with-unit') {
            @trigger_error('bail with unit out', E_USER_ERROR); // The bailout ends the unit with the cycle.
            continue;
        }
        if ($probe === 'destruct-explicit') {
            $ex->__destruct(); // An explicit call does not change a referenced unit.
            $ex->writeBody('explicit-destruct-ok');
            continue;
        }
        if ($probe === 'head204') {
            $ex->writeHead(204);
            $ex->writeBody('dropped-at-seal');
            continue;
        }
        if ($probe === 'exit') {
            $ex->writeBody('bye');
            exit(0); // The host must recycle this worker after it served a request.
        }
        if ($probe === 'spin') {
            // This exchange does not finalize. The renewed unit timeout must stop it.
            while (true) {
            }
        }

        $ex->writeBody('state=' . var_export($ex->isFinalized(), true));
    }
} catch (\Rapira\Exception\ClosedException) {
}

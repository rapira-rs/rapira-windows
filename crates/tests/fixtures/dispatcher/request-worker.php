<?php

// Write each Request field to the response body for the field mapping tests.

$d = \Rapira\get_dispatcher();
try {
    while (true) {
        $ex = $d->receive();
        $req = $ex->getRequest();
        // Repeated calls return the same cached instance.
        $again = $ex->getRequest();
        $caseKeys = [];
        foreach ($req->headers as $k => $v) {
            if (strcasecmp((string)$k, 'x-case') === 0) {
                $caseKeys[] = $k;
            }
        }
        $lines = [
            'method=' . $req->method,
            'uri=' . $req->uri,
            'target-hex=' . bin2hex($req->target),
            'authority=' . var_export($req->authority, true),
            'protocol=' . $req->protocol,
            // Repeated values remain separate list entries in transmission order.
            'x-probe=' . implode('|', $req->headers['x-probe'] ?? []),
            // The same name with two different cases produces two keys. Grouping compares the exact bytes.
            'x-case-keys=' . implode('|', $caseKeys),
            // The symbol table converts an all-digit field name to an integer key.
            'h123=' . implode(',', $req->headers[123] ?? []),
            // A single-letter name remains a string key.
            'h-single=' . implode(',', $req->headers['a'] ?? []),
            // A leading hyphen detects a one-byte overread in the symbol table prefilter.
            'h-dash=' . implode(',', $req->headers['-'] ?? []),
            'h-neg=' . implode(',', $req->headers['-1'] ?? []),
            'memo-same=' . var_export($req === $again, true),
            'body=' . (is_string($req->body) ? $req->body : $req->body::class),
            'remote=' . $req->remote::class,
            'remote-detail=' . ($req->remote instanceof \Rapira\InetAddress
                ? $req->remote->ip . ':' . $req->remote->port
                : var_export($req->remote->path, true)),
            'server=' . $req->server::class,
            'server-detail=' . ($req->server instanceof \Rapira\InetAddress
                ? $req->server->ip . ':' . $req->server->port
                : var_export($req->server->path, true)),
            'tls=' . ($req->tls === null ? 'NULL' : implode('|', [
                $req->tls->version,
                $req->tls->cipher,
                var_export($req->tls->negotiatedProtocol, true),
                var_export($req->tls->requestedServerName, true),
                var_export($req->tls->certSerial, true),
                var_export($req->tls->certOrganization, true),
                var_export($req->tls->certFingerprint, true),
            ])),
            'received-at=' . var_export($req->receivedAt, true),
            'received-at-positive=' . var_export($req->receivedAt > 0, true),
        ];
        $ex->writeBody(implode("\n", $lines));
    }
} catch (\Rapira\Exception\ClosedException) {
}

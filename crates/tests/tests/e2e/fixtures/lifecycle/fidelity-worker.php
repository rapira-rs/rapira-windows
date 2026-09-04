<?php

// Select each test with the request target because dispatcher mode has no superglobals.

$d = \Rapira\get_dispatcher();
try {
	while (true) {
		$ex = $d->receive();
		$req = $ex->getRequest();
		parse_str(parse_url($req->target, PHP_URL_QUERY) ?: "", $q);
		$probe = $q["probe"] ?? "";
		if ($probe === "headers") {
			// The server sends lowercase names. A case-insensitive lookup lets this fixture work with each server implementation.
			$vals = [];
			foreach ($req->headers as $k => $vs) {
				if (strcasecmp((string) $k, "x-probe") === 0) {
					array_push($vals, ...$vs);
				}
			}
			$lines = ["x-probe=" . implode("|", $vals)];
			// A dispatcher pool has no $_SERVER mapping. Names with underscores pass through without changes.
			$lines[] =
				"x_forwarded_for=" .
				implode("|", $req->headers["x_forwarded_for"] ?? []);
			$ex->writeBody(implode("\n", $lines));
		} elseif ($probe === "head") {
			$ex->writeHead(201, [
				"x-a" => ["1", "2"],
				"content-length" => ["999"],
			]);
			$ex->writeBody("body");
		} elseif ($probe === "target") {
			$ex->writeBody(
				implode("\n", [
					"target=" . $req->target,
					"uri=" . $req->uri,
					"authority=" . ($req->authority ?? "null"),
					"host=" . implode("|", $req->headers["host"] ?? []),
				]),
			);
		} elseif ($probe === "received") {
			$ex->writeBody("received=" . var_export($req->receivedAt, true));
		} elseif ($probe === "multipart") {
			$b = $req->body;
			if (!$b instanceof \Rapira\Http\Multipart) {
				$ex->writeBody("not-multipart: " . get_debug_type($b));
				continue;
			}
			$u = $b->files[0];
			$ex->writeBody(
				implode("\n", [
					"field=" . $b->fields[0]->name . "=" . $b->fields[0]->value,
					"file-content=" . file_get_contents($u->tmpPath),
					"tmp=" . $u->tmpPath,
				]),
			);
		} else {
			$ex->writeBody("ok");
		}
	}
} catch (\Rapira\Exception\ClosedException) {
}

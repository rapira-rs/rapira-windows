## Settled, do not reopen

- ZTS PHP 8.4 and 8.5 only. `rapira_sapi.h` rejects NTS headers. Windows 10, Windows 11, and Windows Server on x64. Windows 11 on ARM64.
- One process with a fixed interpreter thread pool per plugin. Each pool owns its work queue. `processes` is the interpreter thread count.
- MINIT runs once before the interpreter threads start. Module teardown runs on the boot thread after every interpreter has stopped. A forced exit uses `TerminateProcess`.
- Use native architecture tools. Build and test x64 and ARM64 on matching native CI runners. Build release PHP from official source with `ci/build-php.ps1`.
- Use console control events for shutdown. Keep fixed pool sizes.
- Foreground only, no daemonize. Pidfile stays.
- Allocator is mimalloc v3.
- New host logic in Rust via ZEND_API if that is reasonable. C only for ZPP shells, longjmp isolation, macro shims.
- Pre 1.0 - do not preserve backwards compatibility.

## PHP contract

- The PHP contract ([rapira-rs/contract](https://github.com/rapira-rs/contract), local checkout `../contract`; update it before you read it) comes first. Read it before you plan, design or change anything that PHP can see: stubs, classes, functions, exceptions, messages and behavior.
- The extension follows the contract. To improve the contract or deviate from it, ask first.

## Comments

- `dev.ps1 -Task stubs` generates each `*_arginfo.h` header under `crates/` from the `*.stub.php` next to it.
- Joke comments (`Rustttt`, "trust me, I'm a developer") are intentional. Do not flag them.

## Tests

- A test proves a behavior from outside, through what a client or an operator sees. Its pass condition is simple: a response, a log record, an exit status or a scoreboard line.
- Adding API for tests only, public or private, is forbidden: no function, method, constructor, accessor, `Default` impl or feature flag in production code that only tests call. A `#[cfg(test)]` gate or a `pub(crate)` visibility does not make one acceptable. No production branch that only a test path takes. A generic parameter or a trait that has one production type and a test type is a test seam. Helpers inside a `#[cfg(test)] mod tests` block are test code, not API.
- If no path from outside reaches a behavior, do not add a hook for it. Test its pure logic in a unit test, or leave it without a test. Drop a test of a sequence that production never runs.
- Assert on the effect that a client sees, not on internal state such as a counter, a queue or a join handle.
- A unit test tests one small piece of code in its own scope, in its crate under `#[cfg(test)] mod tests`. It can call private items. It uses no fixture: no file or directory on disk, no socket, no child process or signal, no environment variable and no PHP. It uses no stand-in that copies another rapira component. A value that the test builds in memory as input (a future, an in-memory writer, a waker, a paused tokio clock) is not a fixture.
- A test that needs a fixture is an e2e test in `crates/tests/tests/e2e/` behind the `e2e` feature, so a workspace run skips it. It spawns the `rapira` binary with the `Spawn` builder in `harness.rs`. A stand-in plugin or a fake PHP is a test double: use a PHP fixture on the real binary. A test that no e2e test can replace is deleted.
- Shared client code is in `crates/tests/src/`: `wire` (an HTTP/1.1 client that returns `Frame`s), `grpc` (gRPC, gRPC-Web and Connect clients), `server_log` (the JSON log readers). Fixtures are in `crates/tests/fixtures/` and `crates/tests/tests/e2e/fixtures/`. No `testing.rs`, `testdata/` or harness `[dev-dependencies]` in a plugin crate. Never the root package's `tests/`.
- Do not assert that a port refuses connections after a stop: another process can bind the free port.
- New tests use worker or dispatcher mode, not classic.
- Check PHP behavior against php-src or a short script rather than guessing.

Use `dev.ps1` for `build`, `test`, `test_e2e`, `coverage`, `stubs`, `grpc_fixtures`, and `clangd`. Pass matching native PHP runtime and development directories. The `clangd` task writes below ignored `target/clangd`.

## Dependencies

Prefer `windows-sys` platform APIs over unnecessary wrappers.

## Docs

- Pre-1.0: no migration framing, no old-to-new tables, no deprecation notes. Docs describe only the current design.

## Known false positives, do not "fix"

- rust-analyzer `E0277: Arguments<'_>: Sync` on `Box::pin` over a `tokio::select!`, while `cargo check` is clean. Cargo is authoritative.
- PHP 8.5 warns that `--enable-opcache` is unrecognized. The flag stays for the 8.4 CI leg.
- Extension visibility differs per CI leg; that is what the `extension_loaded` skip guards are for. Do not edit the test `php.ini`.
- `.clang-tidy` runs in survey mode, so Zend macro signatures trip `bugprone-*`. No CI job runs it.

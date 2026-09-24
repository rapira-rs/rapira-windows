# Core source

Source commit: `a36a356f8bbf9af5fb93c938636372de3c6f062e` in `rapira-rs/rapira`.

The sync follows [the approved plan](docs/plans/2026-09-24-sync-core-main-and-grpc.md). Stage 1 of that plan is complete. The earlier sync to `5f26e1d9` followed [the previous plan](docs/plans/2026-09-02-sync-core-0.8.0.md). Each file that is not listed below matches the source commit, except for STE comment wording and the stub hashes that the generator writes into the three arginfo headers.

## Process model

Rapira for Windows runs one process with a static pool of ZTS PHP interpreter threads. MINIT runs once before the threads start. The core master, the forked worker processes, signals, Unix sockets, the Linux accept loop, and the Docker, nfpm, and Makefile packaging have no Windows counterpart. The `crates/master` crate, `crates/plugins/http/src/accept_linux.rs`, and the `apm`, `classic`, `extensions`, `reload`, and `scaling` end-to-end suites are not ported. The `dispatcher` and `dispatcher_loop` suites exist as `exchange_api` and `exchange_loop` because Windows reports error 740 for an executable name that contains `patch`.

## Workspace and build

| File | Reason |
|---|---|
| `Cargo.toml` | Windows package metadata and version 0.8.0; no master crate; the shared `windows-sys` dependency in place of `libc`. |
| `Cargo.lock` | Resolves the Windows workspace. |
| `crates/*/Cargo.toml` | Version 0.8.0; `windows-sys` in place of `libc`; the `tempfile` and `socket2` test dependencies; the exact tower-http 0.7.1 pin. |
| `crates/php_sys/build.rs` | Compiles the C files for the MSVC toolchain, defines the package version, selects php84 or php85, maps the PHP vectorcall exports to C ABI bridges, generates the DLL data imports, and rejects an NTS PHP. |
| `ci/`, `dev.ps1`, `.clangd` | Native Windows build, test, and release tooling. |

## PHP SAPI (`crates/php_sys`)

| File | Reason |
|---|---|
| `allowed_bindings.rs` | Adds the TSRM functions and the C ABI bridge names; removes the timer bindings that the C shims replace. |
| `module.c` | Resets per-thread C state, sets `SAPI_OPTION_NO_CHDIR` for each thread, contains timer bailouts, and disarms before each arm. No child-process initialization and no PHP 8.6 hunks. |
| `rapira_classes.c`, `rapira_classes.h` | Use `XtOffsetOf` for the embedded object layouts. |
| `rapira_dispatcher.c` | Keeps the request-entry flag per thread and resets it for each interpreter generation. |
| `wrapper.c`, `wrapper.h` | TSRM cache and globals accessors, the SAPI startup bridge that translates Windows log priorities, the thread and timer shims, the hash and interner shims, and the vectorcall bridges. No `rapira_child_init` and no `rapira_init_call_stack`. |
| `src/lib.rs` | Exports `PoolHooks` and declares the thread and timer shims. |
| `src/handler.rs` | Sends boxed jobs through the crossbeam intake so several interpreter threads can consume them. |
| `src/start.rs` | Boots the interpreter thread pool, gates startup, recycles interpreters in place, resets per-thread state, stops interruptible waits, and joins the threads before module teardown. The recycle jitter combines the thread index with a random hasher seed. |
| `src/quota.rs` | Keeps quota, drain, and unhealthy state per interpreter generation. |
| `src/scoreboard.rs` | Clears the draining flag on each generation and aggregates the slots into one snapshot. |
| `src/classic_worker.rs`, `src/rapira_worker.rs` | Take the boxed `Context` directly because the intake has no `Job` wrapper. |
| `src/exchange/mod.rs` | Converts Windows paths to bytes and exposes the dispatcher-cache reset. |
| `src/exchange/receive.rs` | Forgets the cached dispatcher of the interpreter. The C shims contain the timer bailouts. |
| `src/exchange/respond.rs` | Safe functions that use the C timer shims while a frame sender waits. |
| `src/exchange/sendfile.rs` | Rejects a path that is not valid UTF-8 before the Windows canonicalization and containment checks. |
| `src/exchange/tests.rs` | Tests Unicode, invalid UTF-8, and verbatim paths; uses Windows symlinks with a privilege-only skip. |
| `src/types.rs` | No `Job` wrapper. |

## Extension API, runtime, HTTP, and static files

| File | Reason |
|---|---|
| `crates/api/src/prepare.rs` | Binds TCP listeners with default socket options, stores the prepared listener directly, and records each bound address for tests. |
| `crates/runtime/src/lib.rs` | Keeps the `Send` bound on the erased extension, installs the console control handler, and forces the second control event through `TerminateProcess` with code 130. |
| `crates/runtime/src/multipart.rs` | Builds test limits with struct update syntax. |
| `crates/plugins/http/README.md` | Describes the TCP server and the process-wide static cache. |
| `crates/plugins/http/src/lib.rs` | No Unix listener arms and no Linux accept loop. |
| `crates/plugins/http/src/serve.rs` | Consumes the prepared standard listener and classifies the fatal Winsock accept errors. |
| `crates/plugins/http/src/bridge.rs` | Reads file ranges with `seek_read`. |
| `crates/plugins/http/src/check.rs` | Validates the Host field and the absolute-form authority, and accepts field names that php-src accepts. |
| `crates/middleware/static_files/src/lib.rs` | Rejects Windows path aliases before name filtering and gates the symlink-loop proof on privilege error 1314. |
| `crates/middleware/static_files/src/cache.rs` | Provides a test-only clock for the TTL proofs. |

## Configuration and binary

| File | Reason |
|---|---|
| `crates/config/src/lib.rs`, `listen.rs`, `pool.rs`, `supervisor.rs` | Accept only TCP listen addresses, use the Windows pool key set, and describe the drain margin within the runtime timeout budget. |
| `src/main.rs` | Boots one process: validates paths and listeners before PHP starts, probes spool owners through Windows process APIs, keeps the pidfile handle until the shutdown verdict, and calls `TerminateProcess` when the interpreter threads did not join. |
| `src/worker.rs` | Enters the entrypoint directory, starts the interpreter pool, and computes the exit code from the boot-failure hook and the extension outcomes. |
| `src/pidfile.rs` | Creates the pidfile exclusively and keeps its handle until clean shutdown. |
| `src/logging.rs` | Prints the Windows package banner. |
| `src/version_tests.rs` | Verifies one product version across the workspace packages, the CLI, and the PHP API. |

## Tests

| File | Reason |
|---|---|
| `crates/tests/src/lib.rs` | Reads file ranges with `seek_read`, returns an error for a reply slice that ends early, and loads the required DLLs through a process-local scan INI. |
| `crates/tests/tests/exchange_loop.rs` | No Unix address case; uses a file inside the crate as the path outside the sendfile root. |
| `crates/tests/tests/failboot_worker_tests.rs` | Uses a bounded hook to observe the unhealthy threshold before the next interpreter generation resets the slot. |
| `crates/tests/tests/http_values.rs` | Checks an `InetAddress` server value. |
| `crates/tests/tests/php_ext_tests.rs` | Covers the bundled Windows extension set. |
| `crates/tests/tests/e2e/main.rs` | Selects the Windows suites and runs the console-delivery proof first. |
| `crates/tests/tests/e2e/harness.rs` | Uses process groups, checked Ctrl+Break delivery, thread readiness sets, repository examples, and Windows PHP paths. |
| `crates/tests/tests/e2e/lifecycle.rs` | Tests Windows shutdown and exit codes, bind failure, restart, per-thread recycle, memory bounds, and crash backoff. Uses the process ID for every worker because the pool is threads. |
| `crates/tests/tests/e2e/streaming.rs` | Sets the client socket options with socket2. No worker-process death case. |
| `crates/tests/tests/e2e/concurrency.rs`, `ini.rs`, `logging.rs`, `static_files.rs` | Windows thread counts, paths, and banner. |
| `crates/tests/tests/e2e/examples.rs`, `timeout.rs` | Windows-only proofs for the shipped example and the per-thread timer. |
| `examples/` | PowerShell commands and the fixed interpreter thread pool configuration. |

# Core source

Source commit: `522acad23199057fe53b1a3868d50d8f46f9f42f` on the `feature/grpc-connectrpc` branch of `rapira-rs/rapira`. That branch adds the gRPC plugin on top of `main` at `a36a356f8bbf9af5fb93c938636372de3c6f062e`.

The sync follows [the approved plan](docs/plans/2026-09-24-sync-core-main-and-grpc.md). Both stages of that plan are complete. The earlier sync to `5f26e1d9` followed [the previous plan](docs/plans/2026-09-02-sync-core-0.8.0.md). Each file that is not listed below matches the source commit, except for STE comment wording and the stub hashes that the generator writes into the four arginfo headers.

## Process model

Rapira for Windows runs one process with one static pool of ZTS PHP interpreter threads for each configured plugin table. MINIT runs once before the threads start, and one scoreboard holds one slot per thread across the pools. The core master, the forked worker processes, signals, Unix sockets, the Linux accept loop, and the Docker, nfpm, and Makefile packaging have no Windows counterpart. The `crates/master` crate, `crates/plugins/http/src/accept_linux.rs`, and the `apm`, `classic`, `extensions`, `reload`, and `scaling` end-to-end suites are not ported. The `dispatcher`, `dispatcher_loop`, and `grpc_dispatcher` suites exist as `exchange_api`, `exchange_loop`, and `grpc_calls` because Windows reports error 740 for an executable name that contains `patch`.

## Workspace and build

| File | Reason |
|---|---|
| `Cargo.toml` | Windows package metadata and version 0.8.0; no master crate; the shared `windows-sys` dependency in place of `libc`. |
| `Cargo.lock` | Resolves the Windows workspace. |
| `crates/*/Cargo.toml` | Version 0.8.0; `windows-sys` in place of `libc`; the `tempfile` and `socket2` test dependencies; the exact tower-http 0.7.1 pin. |
| `crates/php_sys/build.rs` | Compiles the C files for the MSVC toolchain, defines the package version, selects php84 or php85, maps the PHP vectorcall exports to C ABI bridges, generates the DLL data imports, and rejects an NTS PHP. |
| `ci/`, `dev.ps1`, `.clangd` | Native Windows build, test, fixture, and release tooling. The `grpc_fixtures` task replaces the Makefile target. |
| `crates/tests/fixtures/grpc/` | The descriptor sets match the source bytes. The `echo.proto` comment names the `dev.ps1` task. |

## PHP SAPI (`crates/php_sys`)

| File | Reason |
|---|---|
| `allowed_bindings.rs` | Adds the TSRM functions and the C ABI bridge names; removes the timer bindings that the C shims replace. |
| `module.c` | Resets per-thread C state, sets `SAPI_OPTION_NO_CHDIR` for each thread, reads the run mode of the thread, contains timer bailouts, and disarms before each arm. No child-process initialization and no PHP 8.6 hunks. |
| `rapira_classes.c`, `rapira_classes.h` | Use `XtOffsetOf` for the embedded object layouts. |
| `rapira_dispatcher.c` | Keeps the request-entry flag and the run mode per thread. `rapira_mode_set` and `rapira_mode_get` read and write the thread-local mode, because MSVC thread-local storage is file-static. |
| `wrapper.c`, `wrapper.h` | TSRM cache and globals accessors, the SAPI startup bridge that translates Windows log priorities, the thread, timer, and run-mode shims, the hash and interner shims, and the vectorcall bridges. No `rapira_child_init` and no `rapira_init_call_stack`. |
| `src/lib.rs` | Exports `PoolHooks` and `PoolSpec` and declares the thread, timer, and run-mode shims. |
| `src/handler.rs` | Sends boxed jobs through the crossbeam intake so several interpreter threads can consume them. |
| `src/start.rs` | Boots the module once with `Rapira::boot`, adds one interpreter thread pool per plugin table with `add_pool` on one scoreboard, sets the run mode per thread, gates startup, recycles interpreters in place, resets per-thread state, stops interruptible waits, and joins every pool before module teardown. The recycle jitter combines the thread index with a random hasher seed. |
| `src/quota.rs` | Keeps quota, drain, and unhealthy state per interpreter generation. |
| `src/scoreboard.rs` | Clears the draining flag on each generation and aggregates the slots into one snapshot. |
| `src/classic_worker.rs`, `src/rapira_worker.rs` | Take the boxed `Context` directly because the intake has no `Job` wrapper. |
| `src/exchange/mod.rs` | Converts Windows paths to bytes and exposes the dispatcher-cache reset. |
| `src/exchange/receive.rs` | Forgets the cached dispatcher of the interpreter and reads the run mode of the thread. The C shims contain the timer bailouts. |
| `src/exchange/respond.rs` | Safe functions that use the C timer shims while a frame sender waits. |
| `src/exchange/sendfile.rs` | Rejects a path that is not valid UTF-8 before the Windows canonicalization and containment checks. Keeps the root in a mutex, because one test binary sets several roots. |
| `src/exchange/tests.rs` | Tests Unicode, invalid UTF-8, and verbatim paths; uses Windows symlinks with a privilege-only skip. |
| `src/types.rs` | No `Job` wrapper. |

## Extension API, runtime, network, plugins, and static files

| File | Reason |
|---|---|
| `crates/api/src/prepare.rs` | Binds TCP listeners with default socket options, stores the prepared listener directly, and records each bound address for tests. |
| `crates/runtime/src/lib.rs` | Keeps the `Send` bound on the erased extension, installs the console control handler, fans the first control event out to every extension runtime, and forces the second control event through `TerminateProcess` with code 130. `run` takes no script path: every pool shares the process-wide script paths, so the binary sets them once for the HTTP pool. |
| `crates/runtime/src/multipart.rs` | Builds test limits with struct update syntax. |
| `crates/net/src/lib.rs` | TCP only: the watch-based stop, the prepared standard listener, and the fatal Winsock accept errors. No Unix listener and no Linux accept loop. |
| `crates/plugins/http/README.md` | Describes the TCP server and the process-wide static cache. |
| `crates/plugins/http/src/lib.rs`, `src/serve.rs` | No Unix listener arms. |
| `crates/plugins/grpc/README.md` | Describes the interpreter thread pool, the TCP listener, the console stop, and the `dev.ps1` task. |
| `crates/plugins/grpc/src/serve.rs` | No Unix listener arm. |
| `crates/plugins/http/src/bridge.rs` | Reads file ranges with `seek_read`. |
| `crates/plugins/http/src/check.rs` | Validates the Host field and the absolute-form authority, and accepts field names that php-src accepts. |
| `crates/middleware/static_files/src/lib.rs` | Rejects Windows path aliases before name filtering and gates the symlink-loop proof on privilege error 1314. |
| `crates/middleware/static_files/src/cache.rs` | Provides a test-only clock for the TTL proofs. |

## Configuration and binary

| File | Reason |
|---|---|
| `crates/config/src/lib.rs`, `grpc.rs`, `listen.rs`, `pool.rs`, `supervisor.rs` | Accept only TCP listen addresses, use the Windows pool key set, and describe the drain margin within the runtime timeout budget. |
| `src/main.rs` | Boots one process: validates paths, loads the gRPC schema, and binds every listener before PHP starts, probes spool owners through Windows process APIs, keeps the pidfile handle until the shutdown verdict, and calls `TerminateProcess` when the interpreter threads did not join. |
| `src/worker.rs` | Enters the entrypoint directory of the first pool, sets the script paths of the HTTP pool, boots PHP once, adds one pool per plugin table, serves each extension host on its own thread, stops every host when one host stops, and computes the exit code from the shared boot-failure hook and the extension outcomes. |
| `src/pidfile.rs` | Creates the pidfile exclusively and keeps its handle until clean shutdown. |
| `src/logging.rs` | Prints the Windows package banner. |
| `src/version_tests.rs` | Verifies one product version across the workspace packages, the CLI, and the PHP API. |

## Tests

| File | Reason |
|---|---|
| `crates/tests/src/lib.rs` | Reads file ranges with `seek_read`, returns an error for a reply slice that ends early, and loads the required DLLs through a process-local scan INI. |
| `crates/tests/src/grpc.rs` | TCP only. Reads the listener address from the prepare context. |
| `crates/tests/tests/exchange_loop.rs` | No Unix address case; uses a file inside the crate as the path outside the sendfile root. |
| `crates/tests/tests/extension_tests.rs`, `e2e/streaming.rs` | Set the script paths before `run` for the classic and worker cases. |
| `crates/tests/tests/failboot_worker_tests.rs` | Uses a bounded hook to observe the unhealthy threshold before the next interpreter generation resets the slot. Runs the gRPC failboot case on a scenario thread. |
| `crates/tests/tests/grpc_server.rs` | No Unix listener case. |
| `crates/tests/tests/grpc_calls.rs` | Renamed from `grpc_dispatcher.rs`. No Unix peer case. |
| `crates/tests/tests/http_values.rs` | Checks an `InetAddress` server value. |
| `crates/tests/tests/php_ext_tests.rs` | Covers the bundled Windows extension set. |
| `crates/tests/tests/e2e/main.rs` | Selects the Windows suites and runs the console-delivery proof first. |
| `crates/tests/tests/e2e/harness.rs` | Uses process groups, checked Ctrl+Break delivery, thread readiness sets, repository examples, and Windows PHP paths. Renders the `[grpc]` tables with socket addresses. |
| `crates/tests/tests/e2e/grpc.rs` | Ctrl+Break in place of SIGQUIT. The boot-failure case names the PHP boot. |
| `crates/tests/tests/e2e/lifecycle.rs` | Tests Windows shutdown and exit codes, bind failure, restart, per-thread recycle, memory bounds, and crash backoff. Uses the process ID for every worker because the pool is threads. |
| `crates/tests/tests/e2e/streaming.rs` | Sets the client socket options with socket2. No worker-process death case. |
| `crates/tests/tests/e2e/concurrency.rs`, `ini.rs`, `logging.rs`, `static_files.rs` | Windows thread counts, paths, and banner. |
| `crates/tests/tests/e2e/examples.rs`, `timeout.rs` | Windows-only proofs for the shipped example and the per-thread timer. |
| `examples/` | PowerShell commands and the fixed interpreter thread pool configuration with a commented `[grpc]` block. |

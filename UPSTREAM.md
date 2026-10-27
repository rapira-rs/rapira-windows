# Core source

Core: [`6aa8bbb7cefbbbd1e5eb76410fc5c480b726def2`](https://github.com/rapira-rs/rapira/tree/6aa8bbb7cefbbbd1e5eb76410fc5c480b726def2).

PHP contract: [`27324e79be73fce03ac778dfeec5f00dda716999`](https://github.com/rapira-rs/contract/tree/27324e79be73fce03ac778dfeec5f00dda716999).

The source follows the core plugin architecture and binary-driven test design.

## Shared design

- `crates/sapi` owns PHP startup, interpreter work, base PHP types, and execution modes.
- `crates/php_build` supplies the PHP headers, C compiler settings, and runtime link settings.
- `crates/net` owns prepared listeners. Each plugin owns its configuration and PHP API.
- HTTP supports classic, worker, and dispatcher modes. gRPC supports dispatcher mode and unary gRPC, gRPC-Web, and Connect.
- HTTP dispatcher info exposes the configured server name and port at boot. Shared dispatcher handlers serve both plugins.
- Raw CGI bodies use PHP temporary streams. Worker client aborts end the request and preserve application state.
- Intake entries own byte and slot permits until dequeue. A cancelled queued unit counts as shed and does not use the request quota.
- gRPC supports ordered interceptors and bearer-token authentication. The token file loads before PHP starts.
- Worker and dispatcher boot retries without client traffic. Application work pulls determine readiness. PHP 8.4 and 8.5 expose boot `argv` and `argc` with `register_argc_argv` disabled.
- `rapira serve CONFIG` starts the configured plugin pools. Each plugin has a `<plugin>.pool` table.
- Tests use the server binary for PHP, network, file, and lifecycle behavior. Pure unit tests stay in their crates.

## Windows implementation

- One process initializes ZTS PHP once. Each plugin has a fixed interpreter thread pool and one work queue. The intake has 1024 slots and a 64 MiB body budget per pool. The `processes` key sets its thread count.
- The interpreter keeps its mode, entrypoint, dispatcher, virtual working directory, timer, and request state in thread-local storage. Interpreter recycling retains the pool size.
- Each new interpreter installs the POST reader in its own SAPI table. Process-wide SAPI callbacks are set once after MINIT.
- The default OPcache cache ID is unique to the server process. All its interpreters share the cache.
- The boot thread runs module teardown after all interpreters stop. A shutdown timer covers plugin drain, interpreter teardown, and module teardown with one deadline.
- Ctrl+C or Ctrl+Break starts shutdown. A second control event forces exit. `TerminateProcess` handles forced exit while PHP threads can still be active.
- A locked pidfile identifies the process. Clean shutdown removes it. Forced termination can leave it on disk.
- Listeners use TCP. File responses use Windows offset reads. Static-file checks reject Windows path aliases. Multipart cleanup checks process liveness with Windows APIs.
- Rust links to `php8ts.lib`. C shims isolate Zend bailouts, macros, and the Windows vectorcall ABI. Generated headers come from the PHP stubs.
- Native x64 and ARM64 jobs build and test PHP 8.4 and 8.5. Release packages contain the matching PHP runtime built from official source.
- Observability runs on a dedicated thread with one Tokio IO worker. It reports interpreter states, requests, restarts, pool queues, build versions, liveness, and pool readiness.
- Queue counters belong to the pool. Request and restart counters belong to interpreter slots and survive recycling.

## Platform exclusions

- Unix process supervision, fork cleanup, reload, dynamic scaling, process watchdogs, shared-memory mappings, and transparent huge-page controls.
- Direct NTS Zend globals and PHP 8.7 header changes. The Windows build uses thread-local ZTS accessors with PHP 8.4 and 8.5.
- Unix socket listeners, Unix file permissions, Linux packages, and container builds.
- Per-worker process-exit and RSS/PSS metrics. Windows interpreter threads share one process.
- The cargo-fuzz workflow and fuzz-only public APIs. The native suite covers the supported parser behavior.

The Windows HTTP path calls static-file handling directly. It needs no Tower middleware chain. Windows file reads, path checks, spool cleanup, and socket ownership rules use native APIs.

## Dependencies

`crossbeam-channel` supplies the interpreter queues. Tokio semaphores limit their slots and body bytes. `windows-sys` supplies console control, process liveness, forced exit, and pidfile APIs. `libmimalloc-sys` releases unused allocator pages after an idle interval. The gRPC protocol and protobuf dependencies follow the pinned core source. Both native PHP builds include sodium.

# Core source

Core: [`ac56141af8c1dadf83cd750b7f522b06155d198d`](https://github.com/rapira-rs/rapira/tree/ac56141af8c1dadf83cd750b7f522b06155d198d).

PHP contract: [`fe62f7b0a705638a3e7d14358ea1f4c3e844acb8`](https://github.com/rapira-rs/contract/tree/fe62f7b0a705638a3e7d14358ea1f4c3e844acb8).

The source follows the core plugin architecture and its final binary-driven test design. The design reasoning is in core's `.superpowers/sdd/2026-09-26-plugin-crates/` directory.

## Shared design

- `crates/sapi` owns PHP startup, interpreter work, base PHP types, and execution modes.
- `crates/php_build` supplies the PHP headers, C compiler settings, and runtime link settings.
- `crates/net` owns prepared listeners. Each plugin owns its configuration and PHP API.
- HTTP supports classic, worker, and dispatcher modes. gRPC supports dispatcher mode and unary gRPC, gRPC-Web, and Connect.
- `rapira serve CONFIG` starts the configured plugin pools. Each plugin has a `<plugin>.pool` table.
- Tests use the server binary for PHP, network, file, and lifecycle behavior. Pure unit tests stay in their crates.

## Windows implementation

- One process initializes ZTS PHP once. Each plugin has a fixed interpreter thread pool and a bounded work queue. The `processes` key sets its thread count.
- The interpreter keeps its mode, entrypoint, dispatcher, virtual working directory, timer, and request state in thread-local storage. Interpreter recycling retains the pool size.
- The boot thread runs module teardown after all interpreters stop. A shutdown timer covers plugin drain, interpreter teardown, and module teardown with one deadline.
- Ctrl+C or Ctrl+Break starts shutdown. A second control event forces exit. `TerminateProcess` handles forced exit while PHP threads can still be active.
- A locked pidfile identifies the process. Clean shutdown removes it. Forced termination can leave it on disk.
- Listeners use TCP. File responses use Windows offset reads. Static-file checks reject Windows path aliases. Multipart cleanup checks process liveness with Windows APIs.
- Rust links to `php8ts.lib`. C shims isolate Zend bailouts, macros, and the Windows vectorcall ABI. Generated headers come from the PHP stubs.
- Native x64 and ARM64 jobs build and test PHP 8.4 and 8.5. Release packages contain the matching PHP runtime built from official source.

## Dependencies

`crossbeam-channel` supplies the bounded interpreter queues. `windows-sys` supplies console control, process liveness, forced exit, and pidfile APIs. The gRPC protocol and protobuf dependencies follow the pinned core source.

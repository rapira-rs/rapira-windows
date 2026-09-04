# Core source

Source commit: `5f26e1d9d7974e38824fb0bf2b1c56ca25042f6b` in `rapira/core`.

The sync follows [the approved plan](docs/plans/2026-09-02-sync-core-0.8.0.md). Slices 1 through 7 are implemented. The current slice 7 design adds native ARM64 CI and release builds to the saved plan. The source files came from a read-only export of the specified commit. The local core checkout is at a different commit.

## Slice 1 deltas

| File | Reason |
|---|---|
| `Cargo.toml` | Add the API and scoreboard paths and their shared dependencies; use the workspace edition without a Rust version pin. |
| `Cargo.lock` | Resolve the Windows workspace dependencies for slice 1. |
| `rust-toolchain.toml` | Select stable Rust, rustfmt, and Clippy while leaving the target native to each Windows host. |
| `crates/api/Cargo.toml` | Remove libc from tests; add the approved tempfile dependency for file-read proofs. |
| `crates/api/src/lib.rs` | Prepare before PHP startup; retain the PHP Unix address type; read file ranges with seek_read; test offset and EOF behavior. |
| `crates/api/src/prepare.rs` | Store the prepared TCP listener directly; bind TCP with default socket options; test nonblocking accept, bind conflicts, and rebind with a live connection. |
| `crates/config/src/lib.rs` | Remove Unix-derived server ports and pool scaling exports; test the Windows pool keys and absolute paths; describe the runtime timeout bound. |
| `crates/config/src/listen.rs` | Accept only TCP addresses with explicit ports. |
| `crates/config/src/pool.rs` | Use a static interpreter thread count and remove the five excluded pool keys. |
| `crates/config/src/supervisor.rs` | Describe and test the HTTP drain margin within the runtime timeout budget. |
| `crates/scoreboard/Cargo.toml` | Remove libc. |
| `crates/scoreboard/src/lib.rs` | Use a leaked boxed slot slice and a process-wide Instant clock; document thread indices; test thread visibility and cumulative counters. |
| `UPSTREAM.md` | Record the source commit, file deltas, and verification evidence. |
| `docs/plans/2026-09-02-sync-core-0.8.0.md` | Store the approved Windows sync plan. |

The config manifest matches the specified commit byte for byte. The API middleware, HTTP config, and log config have comment-only deltas listed in the comment audit section.

## Slice 2 deltas

| File | Reason |
|---|---|
| `crates/php_sys/allowed_bindings.rs` | Add the four Windows TSRM functions to the core bindings. |
| `crates/php_sys/build.rs` | Compile all six C files, define the package version, select php84 or php85, and track the copied inputs. |
| `crates/php_sys/module.c` | Reset per-thread C state, contain timer bailouts, disarm before each arm, and remove child-process initialization. |
| `crates/php_sys/rapira_dispatcher.c` | Keep request-entry state per thread and reset it for each interpreter generation. |
| `crates/php_sys/wrapper.c` | Keep the Windows TSRM cache and globals accessors; add core enum, array, string, and version shims. |
| `crates/php_sys/wrapper.h` | Keep Windows bindgen rewrites; add core types and the thread/timer shim declarations. |
| `crates/tests/tests/e2e/timeout.rs` | Define the Windows timeout and recycle proofs before timer implementation. |
| `crates/tests/tests/e2e/fixtures/timeout/budget-worker.php` | Supply an opcode-bound worker loop for the timeout proof. |
| `crates/tests/tests/e2e/fixtures/timeout/recycle-worker.php` | Supply an armed timer and work within the next interpreter's budget. |
| `dev.ps1` | Add the planned tasks, validate native PHP, LLVM, and MSVC inputs, select the matching Rust target, and propagate native exit codes. |

The companion C, header, stub, and generated-header files were copied and compared with the source export before edits. Their current comment-only and generated stub-hash deltas are listed in the comment audit section.

## Slice 3 deltas

The complete Rust source directory was copied first. The initial directory diff was empty and all file hashes matched. The current comment-only deltas are listed in the comment audit section.

| File | Reason |
|---|---|
| `crates/php_sys/Cargo.toml` | Use the shared scoreboard, tracing, Tokio runtime features, and a crossbeam channel for multiple intake consumers. |
| `crates/php_sys/src/lib.rs` | Export PoolHooks and declare the Windows thread and timer shims. |
| `crates/php_sys/src/handler.rs` | Send jobs through the shared crossbeam intake. |
| `crates/php_sys/src/scoreboard.rs` | Clear the draining flag on each generation and test that totals survive the reset. |
| `crates/php_sys/src/start.rs` | Boot a ZTS thread pool, gate startup, recycle interpreters, reset state, stop interruptible waits, and join before module teardown; test pool policy and timer cancellation. |
| `crates/php_sys/src/quota.rs` | Keep quota, drain, and unhealthy state per generation; mark requests handled by PHP separately from shed responses. |
| `crates/php_sys/src/exchange/mod.rs` | Convert Windows paths to bytes and expose the dispatcher-cache reset and timer wrappers. |
| `crates/php_sys/src/exchange/receive.rs` | Forget the interpreter's cached dispatcher and state that C contains timer bailouts. |
| `crates/php_sys/src/exchange/respond.rs` | Use the C timer wrappers while a frame sender waits. |
| `crates/php_sys/src/exchange/sendfile.rs` | Reject non-UTF-8 paths before Windows canonicalization and containment checks. |
| `crates/php_sys/src/exchange/tests.rs` | Test Unicode, invalid UTF-8, verbatim paths, and containment; use Windows symlinks with a privilege-only skip. |
| `crates/php_sys/allowed_bindings.rs` | Remove direct timer bindings after their Rust callers moved to C wrappers; include the cdecl interner carrier. |
| `crates/php_sys/build.rs` | Map seven PHP vectorcall functions to C ABI bridges and generate the DLL data import and interner alias. |
| `crates/php_sys/wrapper.c` | Bridge PHP vectorcall exports and the live interner function pointer to the C ABI. |
| `crates/php_sys/wrapper.h` | Declare the ABI bridges with the exact PHP 8.4 or 8.5 signatures. |

The generation-state test failed against the copied core scoreboard code: the slot stayed in state 4 (draining) instead of state 2 (idle). It passed after sb_set cleared the flag. The test also checks cumulative handled and recycle totals. An isolated test harness ran this proof before the remaining PHP pool code could compile.

The first linked test exposed nine unresolved PHP symbols. Seven functions use Windows vectorcall exports. The interner is also a vectorcall function pointer. C ABI bridges keep the core Rust callers unchanged. The Throwable class pointer needs an explicitly linked DLL extern declaration. Bindgen 0.72.1 does not emit a link-name override for an ordinary extern variable, so the interner uses a generated shim declaration and a Rust re-export.

Core counts shed responses in the handled metric. A separate internal flag records whether PHP handled a request, so shed 503 responses cannot suppress the initial boot-failure hook. The flag survives interpreter generations. Startup tokens prevent partially created pools from entering PHP. Each successful start still runs its first bootstrap when shutdown follows immediately.

Both PHP 8.4 and PHP 8.5 pass the locked php_sys check and all 53 unit tests. The timer regression includes a positive timer-fire control and observes cancellation before thread memory is freed. Removing the one production disarm call makes the test fail with the timer-fired state. Restoring the exact source makes it pass. The test also serves a job after recycle and shuts down with a retained handle. The two symlink checks skip on this VM because Windows reports error 1314.

The full workspace build and tests return 101 in the old API callers scheduled for slices 4 and 5. Formatting and whitespace checks pass. The five static-path proofs for slice 4 are prepared outside the workspace; all five fail against the exact core eligible method.

The installed PHP 8.4.25 and 8.5.10 packs both contain php_zend_test.dll, contrary to the plan's pack inventory. The observer suites remain outside the agreed scope. PHP 8.4 loads OPcache from a separate DLL; PHP 8.5 loads it without that DLL.

## Slice 4 deltas

All runtime, HTTP, and static-file files were copied and compared with the approved core export before edits. The HTTP accept loop, TCP accept arm, connection spawn, and drain logic retain the core code. The current comment-only deltas are listed in the comment audit section.

| File | Reason |
|---|---|
| `Cargo.toml` | Replace the old host and Pingora members with runtime and HTTP; add static-file middleware. |
| `Cargo.lock` | Resolve hyper, hyper-util, and the exact tower-http 0.7.1 dependency. |
| `crates/runtime/Cargo.toml` | Remove libc and use the shared windows-sys 0.61 console and process APIs. |
| `crates/runtime/src/lib.rs` | Use the retained Windows console handler and Running::serve; report a handler installation failure; force the second event through TerminateProcess with code 130. |
| `crates/plugins/http/Cargo.toml` | Remove libc and use the core hyper dependencies. |
| `crates/plugins/http/README.md` | Describe the TCP server and the Windows listener behavior. |
| `crates/plugins/http/src/lib.rs` | Remove Unix listener configuration arms. |
| `crates/plugins/http/src/serve.rs` | Consume the prepared standard listener and classify fatal Winsock errors; test accept and immediate rebind with a live connection. |
| `crates/plugins/http/src/bridge.rs` | Read file ranges with seek_read. |
| `crates/middleware/static_files/Cargo.toml` | Remove libc and pin tower-http to 0.7.1. |
| `crates/middleware/static_files/src/lib.rs` | Reject Windows path aliases before name filtering; test five bypasses; gate the symlink-loop proof on privilege error 1314; and advance the test cache clock in TTL proofs. |
| `crates/middleware/static_files/src/cache.rs` | Describe the 16 MiB process-wide cache shared by interpreter threads and provide a test-only manual clock for deterministic TTL proofs. |

The three crate checks pass. All 94 unit tests pass: 40 HTTP, 16 runtime, and 38 static-file tests. The five path regression tests failed against the copied core method and pass with the Windows filter. The symlink-loop test returns early on this VM because Windows denies symlink creation with error 1314. Formatting and whitespace checks pass. The workspace binary still requires slice 5.

The old extension_host and plugins/pingora files are removed. Their core replacements use the approved crate paths and names.

## Slice 5 deltas

The root manifest, main, worker, logging, and pidfile files were copied from the approved export and compared before edits. The pidfile file remains byte-identical to core's master implementation. Logging has a comment-only delta listed in the comment audit section.

| File | Reason |
|---|---|
| `Cargo.toml` | Use Windows package metadata and version 0.8.0; remove master, libc, log, and env_logger; share the approved Windows process API dependency and core tracing dependencies. |
| `Cargo.lock` | Resolve the final binary dependencies. |
| `src/main.rs` | Use the core CLI and single-process boot order; validate paths and listeners before PHP starts; probe spool owners through Windows process APIs; guard owned process handles; reclaim the current PID directory after PID reuse; preserve the pidfile until the shutdown verdict. |
| `src/version_tests.rs` | Verify one product version across the workspace packages, CLI, and PHP API. |
| `src/worker.rs` | Wire the static PHP pool, upload limits, boot-failure stopper, bounded shutdown, exit codes, and joined-only spool cleanup; log extension failures and retain the console guard. |
| `src/pidfile.rs` | Place the unchanged master pidfile guard beside the Windows binary. |
| `crates/runtime/src/lib.rs` | Return a Served holder so console handling stays active through PHP shutdown and the binary's forced-exit decision; report a failed SetConsoleCtrlHandler call. |

The spool probe proof was saved before implementation. It covers invalid names, the current PID, a live process, an exited child, OpenProcess errors, query errors, and STILL_ACTIVE. Four worker proofs cover boot-failure precedence, runtime failures, and the stopper-registration race. PHP 8.4 and PHP 8.5 each pass the full binary build, all 17 binary unit tests, and all 16 runtime unit tests. A PHP 8.5 server with two interpreter threads answered a live request with HTTP 200 and body `ok`. Formatting and whitespace checks pass.

Review found that the retained Windows watcher originally ended when extension serving returned. The PHP join can still have live threads at that point. A prewritten lifecycle test covers a second control event during that join. The Served holder now keeps the watcher through PHP shutdown while allowing the Tokio runtime to drop first. The main function calls TerminateProcess before either the holder or pidfile can drop when joining fails.

The workspace test run returns 101 in the old smoke test. That test still passes `--threads` and expects the old PHP API. Slice 6 replaces it. The Windows lifecycle proofs for exits 0, 1, 70, and 130, port conflict, and restart are saved before the harness port.

## Slice 6 deltas

The tests crate uses the core path and package name. The 16 in-process suites and 141 fixtures were copied and hash-checked before edits. The old integration_tests crate is removed. The current comment-only fixture and test deltas are listed in the comment audit section.

| File | Reason |
|---|---|
| `Cargo.toml` | Replace the old integration test member with crates/tests. |
| `Cargo.lock` | Resolve the renamed test package and its Windows dependencies. |
| `dev.ps1` | Use the lockfile for build, test, e2e, and coverage tasks. |
| `crates/tests/Cargo.toml` | Use the shared Windows console and process APIs and gate the server tests behind the e2e feature. |
| `crates/tests/src/lib.rs` | Read file ranges with seek_read and load required DLLs through a process-local scan INI while preserving fixture PHPRC files. |
| `crates/tests/tests/exchange_api.rs` | Rename the dispatcher API suite to avoid the observed Windows error 740 for its original executable name. |
| `crates/tests/tests/exchange_loop.rs` | Rename the dispatcher loop suite to avoid the observed Windows error 740 for its original executable name; remove the Unix address case and use an existing Windows file outside the sendfile root. |
| `crates/tests/tests/http_values.rs` | Check an InetAddress server value. |
| `crates/tests/fixtures/http_values/construct.php` | Construct the server address with InetAddress. |
| `crates/tests/tests/php_ext_tests.rs` | Remove the two IMAP tests outside the bundled extension set. |
| `crates/php_sys/build.rs` | Map sapi_startup to the Windows C priority bridge. |
| `crates/php_sys/wrapper.c` | Translate native error and warning priorities before the unchanged Rust log callback. |
| `crates/php_sys/wrapper.h` | Declare the SAPI startup bridge with PHP's exact signature. |
| `crates/tests/tests/e2e/main.rs` | Select the approved Windows suites, place the console-delivery proof first, and run the shipped example proof. |
| `crates/tests/tests/e2e/harness.rs` | Use generated process-creation flags, process groups, checked Ctrl+Break delivery, a console probe, taskkill cleanup, thread readiness sets, repository examples, and Windows PHP extension and INI paths. |
| `crates/tests/tests/e2e/examples.rs` | Run the shipped Fiber example and verify its complete streamed response. |
| `crates/tests/tests/e2e/lifecycle.rs` | Test Windows shutdown and exit codes, bind failure, restart, per-thread recycle, memory bounds, and interruptible crash backoff. |
| `crates/tests/tests/e2e/fixtures/lifecycle/windows-lifecycle-worker.php` | Hold requests and PHP thread teardown at observable lifecycle points. |
| `crates/tests/tests/e2e/fixtures/lifecycle/quota-worker.php` | Keep queued work active across four interpreter threads. |
| `crates/tests/tests/e2e/fixtures/lifecycle/memory-worker.php` | Retain a known allocation per request and report PHP memory use across interpreter generations. |
| `crates/tests/tests/e2e/concurrency.rs` | Run the request-isolation proof with eight interpreter threads. |
| `crates/tests/tests/e2e/ini.rs` | Verify explicit PHPRC-file precedence with spaces in the Windows path. |
| `crates/tests/tests/e2e/logging.rs` | Check the Windows package banner. |
| `crates/tests/tests/e2e/static_files.rs` | Quote Windows paths and remove Unix permission cases while retaining serving and invalid-root tests. |
| `crates/tests/tests/e2e/streaming.rs` | Remove the excluded worker-process death case. |
| `crates/tests/tests/failboot_worker_tests.rs` | Use a bounded hook to observe the unhealthy threshold before the next Windows interpreter generation resets the slot. |

Focused slice 6 proofs cover console delivery, lifecycle exits, request isolation, timer cancellation, C-runtime INI propagation, native severity translation, and the boot-failure threshold. Windows PHP defines LOG_ERR as 4 and LOG_WARNING as 5, so the C SAPI startup bridge translates them to the values expected by the unchanged Rust callback. Windows maps LOG_NOTICE, LOG_INFO, and LOG_DEBUG to 6, so the bridge preserves 6.

The busy-loop mutation proof restores the old timer platform guard in module.c and rebuilds the server. The timeout test then returns 101 because the PHP loop never reaches its one-second limit. Restoring the exact source bytes and rebuilding makes the same test pass. The earlier interpreter-teardown mutation independently proves that the production disarm call cancels an armed timer before thread memory is freed.

The generated extension INI reaches both the Win32 environment and PHP's UCRT `getenv` view. The complete PHP 8.4 and 8.5 suite runs in the matching native CI jobs.

## Developer tooling and native hosts

The formatter and tidy policy were copied from the live core checkout at `f9644c449684bca8262e81f5a83659cbe2ca7fbb`. Core has no `.clangd` file or compilation-database generator. The approved source export has the same formatter and tidy policy.

| File | Reason |
|---|---|
| `.clang-format` | Use core's LLVM-derived four-space C formatting policy. |
| `.clang-format-ignore` | Exclude all three generated arginfo headers from formatter churn. |
| `.clang-tidy` | Use core's checks with a header filter that accepts both Windows and slash-separated paths. |
| `.clangd` | Read the ignored database from `target/clangd`, parse with clang-cl, and suppress unreliable unused-include diagnostics from PHP umbrella headers. |
| `dev.ps1` | Add the clangd task; generate six machine-local commands with the native OS target, PHP headers, LLVM, and MSVC headers; and reject emulated PowerShell or non-native PHP, LLVM, and MSVC inputs. |
| `rust-toolchain.toml` | Let Rust use the host target for local tasks. |

`dev.ps1 -Devel <native-devel-directory> -Task clangd` writes absolute native PHP, LLVM, and MSVC paths only below ignored `target/clangd`. It does not require a PHP runtime or export the PHP linker environment. `.clangd` selects that database, parses with clang-cl, and suppresses unreliable unused-include diagnostics from PHP umbrella headers. The tidy configuration passes LLVM 22 config validation.

`ci/build-php.ps1` builds a native ZTS runtime and development tree from verified official PHP 8.4 or 8.5 source. It checks native PowerShell and build tools, PE machine values, headers, PHP identity, required modules, and the PHP license before exporting the Rapira build environment. CI builds and tests ARM64 with this helper. Release jobs use it for both x64 and ARM64. Local work uses only tools and PHP files for the host architecture.

## Slice 7 deltas

| File | Reason |
|---|---|
| `.github/dependabot.yml` | Retain Cargo and GitHub Actions updates and omit Docker updates. |
| `.github/ISSUE_TEMPLATE/bug-report.yml` | Collect the Rapira, PHP, and Windows versions needed for a Windows defect. |
| `.github/ISSUE_TEMPLATE/chore.yml` | Use the Windows repository's maintenance request fields and labels. |
| `.github/ISSUE_TEMPLATE/config.yml` | Link the Windows README and Windows repository discussions. |
| `.github/ISSUE_TEMPLATE/feature-request.yml` | Use concise problem and proposed-behavior fields for feature requests. |
| `.github/pull_request_template.md` | Use the Windows repository review checklist and contribution terms. |
| `.github/workflows/ci.yml` | Resolve PHP 8.4 and 8.5 once, test x64 with verified official Windows packages, build PHP for native ARM64 tests, and keep coverage on the native x64 lane. |
| `.github/workflows/build-binaries.yml` | Build PHP and Rapira for PHP 8.4 and 8.5 on native x64 and ARM64 runners, smoke each bundle, and create architecture-specific archives and checksums. |
| `.github/workflows/nightly.yml` | Read the root Cargo package version and refresh a rolling Windows prerelease from a successful main CI run without Docker publishing. |
| `.github/workflows/release-please.yml` | Build the four native Windows archives after Release Please creates a tag and publish only after every bundle is uploaded. |
| `.github/workflows/clippy.yml` | Omit the Unix lint workflow because native formatting and Clippy checks run in `ci.yml`. |
| `.github/workflows/coverage.yml` | Omit the Unix coverage workflow because Windows coverage runs in `ci.yml`. |
| `.github/workflows/docker.yml` | Omit Docker image builds from the Windows-only release pipeline. |
| `.github/workflows/master.yml` | Omit the Unix php-src master job from the Windows PHP 8.4 and 8.5 matrix. |
| `.github/php-configure-flags.txt` | Replace the Unix PHP build profile with `ci/php-configure-flags.txt`. |
| `.dockerignore` | Omit Docker build-context rules because this repository does not publish container images. |
| `.gitignore` | Ignore generated builds, release staging, machine-local Cargo configuration, coverage output, and MSVC debug files. |
| `.release-please-manifest.json` | Start the Windows package release line at version 0.1.0. |
| `.zed/debug.json` | Omit the Unix LLDB fork-debug profile and its Unix library paths. |
| `README.md` | Link to the shared documentation and describe only Windows platform, package, process, file, socket, and build differences. |
| `CONTRIBUTING.md` | Document native x64 and ARM64 source builds, tests, clangd, and Windows releases. |
| `AGENTS.md` | Apply the live core writing, comment, test, dependency, documentation, and review rules while retaining the Windows settled design. |
| `Dockerfile` | Omit the Unix container payload because releases are native Windows archives. |
| `LICENSE` | Retain the Windows repository copyright notice. |
| `Makefile` | Omit Unix build tasks because `dev.ps1` is the Windows task entry point. |
| `nfpm.yaml` | Omit Linux package definitions because releases are Windows archives. |
| `ci/resolve-php.ps1` | Resolve one exact PHP patch release with verified Windows package and official source metadata. |
| `ci/provision-php.ps1` | Provision and validate the official x64 ZTS runtime and development package for x64 CI. |
| `ci/build-php.ps1` | Build, validate, reuse, and export a native ZTS PHP runtime and development tree from verified source. |
| `ci/php-configure-flags.txt` | Define the dependency-free Windows ZTS PHP profile shared by x64 and ARM64 source builds. |
| `ci/php-src-8.4-exif.patch` | Make PHP 8.4's EXIF dependency on mbstring optional so static EXIF can build with shared mbstring. |
| `ci/php-src-8.5-arm64.patch` | Select PHP's scalar fallback for the unsupported MSVC ARM64 SIMD compatibility path in PHP 8.5. |
| `ci/windows-extensions.txt` | Require the fileinfo and mbstring DLLs produced by both native source builds. |
| `examples/rapira.toml` | Use Windows paths and the fixed interpreter thread pool configuration. |
| `examples/README.md` | Use PowerShell commands and explain Windows interpreter threads and the one-active-exchange rule. |
| `examples/dispatcher-async.php` | Complete the active Fiber before receiving the next exchange. |

`.clang-format`, `codecov.yml`, `release-please-config.json`, and `rust-toolchain.toml` match the specified core commit byte for byte. The classic, synchronous dispatcher, and worker example scripts have comment-only deltas listed in the comment audit section. `AGENTS.md` applies the compatible instruction sections from live core commit `f9644c449684bca8262e81f5a83659cbe2ca7fbb` and retains the Windows settled design.

The Release Please configuration also matches live core commit `f9644c449684bca8262e81f5a83659cbe2ca7fbb` byte for byte. The Windows workflows use the base release pull request, draft release, stable release, nightly release, and manual recovery behavior. Each published release contains four native Windows archives and two architecture checksum files.

All workspace packages use version 0.8.0. The root package version sets the Nightly artifact version. The release manifest records the last released version.

## Comment audit deltas

The comment audit applies the writing rules from live core commit `f9644c449684bca8262e81f5a83659cbe2ca7fbb`. The files in the following lists previously matched source commit `5f26e1d9d7974e38824fb0bf2b1c56ca25042f6b`. They now differ only in comments. Files that already have a Windows delta are not repeated.

### Rust source comments

- `crates/api/src/middleware.rs`: The copied comments use STE wording.
- `crates/config/src/http.rs`: The copied comments use STE wording.
- `crates/config/src/log.rs`: The copied comments use STE wording.
- `crates/php_sys/src/callbacks.rs`: The copied comments use STE wording.
- `crates/php_sys/src/classic_worker.rs`: The copied comments use STE wording.
- `crates/php_sys/src/context.rs`: The copied comments use STE wording.
- `crates/php_sys/src/diagnostics.rs`: The copied comments use STE wording.
- `crates/php_sys/src/dispatcher.rs`: The copied comments use STE wording.
- `crates/php_sys/src/executor.rs`: The copied comments use STE wording.
- `crates/php_sys/src/fold.rs`: The copied comments use STE wording.
- `crates/php_sys/src/module.rs`: The copied comments use STE wording.
- `crates/php_sys/src/rapira_worker.rs`: The copied comments use STE wording.
- `crates/php_sys/src/types.rs`: The copied comments use STE wording.
- `crates/php_sys/src/values.rs`: The copied comments use STE wording.
- `crates/php_sys/src/zend.rs`: The copied comments use STE wording.
- `crates/php_sys/src/exchange/headers.rs`: The copied comments use STE wording.
- `crates/php_sys/src/exchange/request.rs`: The copied comments use STE wording.
- `crates/plugins/http/src/check.rs`: The copied comments use STE wording.
- `crates/plugins/http/src/handler.rs`: The copied comments use STE wording.
- `crates/plugins/http/src/request.rs`: The copied comments use STE wording.
- `crates/plugins/http/src/response.rs`: The copied comments use STE wording.
- `crates/runtime/src/multipart.rs`: The copied comments use STE wording.
- `crates/tests/tests/app_logger.rs`: The copied comments use STE wording.
- `crates/tests/tests/app_logger_limits.rs`: The copied comments use STE wording.
- `crates/tests/tests/app_logger_types.rs`: The copied comments use STE wording.
- `crates/tests/tests/basic_tests.rs`: The copied comments use STE wording.
- `crates/tests/tests/extension_tests.rs`: The copied comments use STE wording.
- `crates/tests/tests/failboot_tests.rs`: The copied comments use STE wording.
- `crates/tests/tests/general_tests.rs`: The copied comments use STE wording.
- `crates/tests/tests/mode.rs`: The copied comments use STE wording.
- `crates/tests/tests/ported_tests.rs`: The copied comments use STE wording.
- `crates/tests/tests/worker_mode.rs`: The copied comments use STE wording.
- `src/logging.rs`: The copied comments use STE wording.

### C and header comments

- `crates/php_sys/rapira_classes.c`: The copied comments use STE wording.
- `crates/php_sys/rapira_classes.h`: The copied comments use STE wording.
- `crates/php_sys/rapira_exchange.c`: The copied comments use STE wording.
- `crates/php_sys/rapira_http.c`: The copied comments use STE wording.

### Other comments

- `crates/php_sys/rapira.stub.php`: The copied comments use STE wording.
- `crates/php_sys/rapira_exception.stub.php`: The copied comments use STE wording.
- `crates/php_sys/rapira_http.stub.php`: The copied comments use STE wording.
- `crates/tests/fixtures/app_logger/app-logger-exception.php`: The copied comments use STE wording.
- `crates/tests/fixtures/app_logger/app-logger-exit-in-serializer.php`: The copied comments use STE wording.
- `crates/tests/fixtures/app_logger/app-logger-levels.php`: The copied comments use STE wording.
- `crates/tests/fixtures/app_logger/app-logger-throwing-serializer.php`: The copied comments use STE wording.
- `crates/tests/fixtures/app_logger/app-logger-unencodable.php`: The copied comments use STE wording.
- `crates/tests/fixtures/app_logger/limits-cycles.php`: The copied comments use STE wording.
- `crates/tests/fixtures/app_logger/limits-deep.php`: The copied comments use STE wording.
- `crates/tests/fixtures/app_logger/limits-huge-string.php`: The copied comments use STE wording.
- `crates/tests/fixtures/app_logger/limits-large-array.php`: The copied comments use STE wording.
- `crates/tests/fixtures/app_logger/types-objects.php`: The copied comments use STE wording.
- `crates/tests/fixtures/app_logger/types-scalars.php`: The copied comments use STE wording.
- `crates/tests/fixtures/basic_tests/boot-output-worker.php`: The copied comments use STE wording.
- `crates/tests/fixtures/basic_tests/error-levels-worker.php`: The copied comments use STE wording.
- `crates/tests/fixtures/basic_tests/fibers.php`: The copied comments use STE wording.
- `crates/tests/fixtures/basic_tests/finish-request-bailout-worker.php`: The copied comments use STE wording.
- `crates/tests/fixtures/basic_tests/h2-boot-bail-worker.php`: The copied comments use STE wording.
- `crates/tests/fixtures/basic_tests/last-error-worker.php`: The copied comments use STE wording.
- `crates/tests/fixtures/basic_tests/warn-after-loop-worker.php`: The copied comments use STE wording.
- `crates/tests/fixtures/dispatcher/host-created-only.php`: The copied comments use STE wording.
- `crates/tests/fixtures/dispatcher/multipart-worker.php`: The copied comments use STE wording.
- `crates/tests/fixtures/dispatcher/not-in-dispatcher-mode.php`: The copied comments use STE wording.
- `crates/tests/fixtures/dispatcher/poll-worker.php`: The copied comments use STE wording.
- `crates/tests/fixtures/dispatcher/recv-probes-worker.php`: The copied comments use STE wording.
- `crates/tests/fixtures/dispatcher/request-worker.php`: The copied comments use STE wording.
- `crates/tests/fixtures/dispatcher/stream-worker.php`: The copied comments use STE wording.
- `crates/tests/fixtures/dispatcher/verbs-worker.php`: The copied comments use STE wording.
- `crates/tests/fixtures/dispatcher/worker-singleton.php`: The copied comments use STE wording.
- `crates/tests/fixtures/extension_tests/ext-driver-classic.php`: The copied comments use STE wording.
- `crates/tests/fixtures/extension_tests/ext-driver-worker.php`: The copied comments use STE wording.
- `crates/tests/fixtures/failboot_worker_tests/failboot-worker.php`: The copied comments use STE wording.
- `crates/tests/fixtures/general_tests/abort-ignore-worker.php`: The copied comments use STE wording.
- `crates/tests/fixtures/general_tests/abort-worker.php`: The copied comments use STE wording.
- `crates/tests/fixtures/general_tests/fatal-backtrace-worker.php`: The copied comments use STE wording.
- `crates/tests/fixtures/general_tests/resources-worker.php`: The copied comments use STE wording.
- `crates/tests/fixtures/general_tests/stuck-flag-worker.php`: The copied comments use STE wording.
- `crates/tests/fixtures/general_tests/throw-quiet-worker.php`: The copied comments use STE wording.
- `crates/tests/fixtures/ini/shared/php.ini`: The copied comments use STE wording.
- `crates/tests/fixtures/mode/dispatcher.php`: The copied comments use STE wording.
- `crates/tests/fixtures/mode/worker.php`: The copied comments use STE wording.
- `crates/tests/fixtures/php_ext/browscap-worker.php`: The copied comments use STE wording.
- `crates/tests/fixtures/php_ext/ctype-worker.php`: The copied comments use STE wording.
- `crates/tests/fixtures/php_ext/fileinfo-worker.php`: The copied comments use STE wording.
- `crates/tests/fixtures/php_ext/filter-worker.php`: The copied comments use STE wording.
- `crates/tests/fixtures/php_ext/iconv-worker.php`: The copied comments use STE wording.
- `crates/tests/fixtures/php_ext/opcache-worker.php`: The copied comments use STE wording.
- `crates/tests/fixtures/php_ext/openssl-worker.php`: The copied comments use STE wording.
- `crates/tests/fixtures/php_ext/sqlite3-worker.php`: The copied comments use STE wording.
- `crates/tests/fixtures/ported_tests/bad-header-worker.php`: The copied comments use STE wording.
- `crates/tests/fixtures/ported_tests/boot-global-worker.php`: The copied comments use STE wording.
- `crates/tests/fixtures/ported_tests/boot-shutdown-worker.php`: The copied comments use STE wording.
- `crates/tests/fixtures/ported_tests/dtor-throw-shutdown-worker.php`: The copied comments use STE wording.
- `crates/tests/fixtures/ported_tests/file-stream-worker.php`: The copied comments use STE wording.
- `crates/tests/fixtures/ported_tests/job-shutdown-worker.php`: The copied comments use STE wording.
- `crates/tests/fixtures/ported_tests/late-shutdown-worker.php`: The copied comments use STE wording.
- `crates/tests/fixtures/ported_tests/preloop-session-handler-worker.php`: The copied comments use STE wording.
- `crates/tests/fixtures/ported_tests/session-handler-worker.php`: The copied comments use STE wording.
- `crates/tests/fixtures/ported_tests/shutdown-fatal-boot-worker.php`: The copied comments use STE wording.
- `crates/tests/fixtures/shared/bailout-worker.php`: The copied comments use STE wording.
- `crates/tests/fixtures/shared/error-keeps-headers.php`: The copied comments use STE wording.
- `crates/tests/fixtures/shared/fibers-worker.php`: The copied comments use STE wording.
- `crates/tests/fixtures/shared/finish-request-worker.php`: The copied comments use STE wording.
- `crates/tests/fixtures/shared/leak-worker.php`: The copied comments use STE wording.
- `crates/tests/fixtures/shared/session-bailout-worker.php`: The copied comments use STE wording.
- `crates/tests/fixtures/shared/shutdown-fatal-worker.php`: The copied comments use STE wording.
- `crates/tests/fixtures/shared/teardown-bailout-worker.php`: The copied comments use STE wording.
- `crates/tests/fixtures/worker/drain-worker.php`: The copied comments use STE wording.
- `crates/tests/fixtures/worker/env-worker.php`: The copied comments use STE wording.
- `crates/tests/fixtures/worker/exit-worker.php`: The copied comments use STE wording.
- `crates/tests/fixtures/worker/gate-classic.php`: The copied comments use STE wording.
- `crates/tests/fixtures/worker/gate-dispatcher-worker.php`: The copied comments use STE wording.
- `crates/tests/fixtures/worker/nested-worker.php`: The copied comments use STE wording.
- `crates/tests/fixtures/worker/never-loop-worker.php`: The copied comments use STE wording.
- `crates/tests/fixtures/worker/one-turn-worker.php`: The copied comments use STE wording.
- `crates/tests/tests/e2e/fixtures/ini/precision.php`: The copied comments use STE wording.
- `crates/tests/tests/e2e/fixtures/lifecycle/bad-header-worker.php`: The copied comments use STE wording.
- `crates/tests/tests/e2e/fixtures/lifecycle/fatal-worker.php`: The copied comments use STE wording.
- `crates/tests/tests/e2e/fixtures/lifecycle/fiber-worker.php`: The copied comments use STE wording.
- `crates/tests/tests/e2e/fixtures/lifecycle/fidelity-worker.php`: The copied comments use STE wording.
- `crates/tests/tests/e2e/fixtures/lifecycle/hang-worker.php`: The copied comments use STE wording.
- `crates/tests/tests/e2e/fixtures/lifecycle/never-loop-worker.php`: The copied comments use STE wording.
- `crates/tests/tests/e2e/fixtures/lifecycle/repeated-headers-worker.php`: The copied comments use STE wording.
- `crates/tests/tests/e2e/fixtures/lifecycle/status-1xx-worker.php`: The copied comments use STE wording.
- `crates/tests/tests/e2e/fixtures/lifecycle/status-header-worker.php`: The copied comments use STE wording.
- `crates/tests/tests/e2e/fixtures/lifecycle/stream-worker.php`: The copied comments use STE wording.
- `crates/tests/tests/e2e/fixtures/lifecycle/upload-worker.php`: The copied comments use STE wording.
- `crates/tests/tests/e2e/fixtures/shared/echo-worker.php`: The copied comments use STE wording.
- `examples/classic.php`: The copied comments use STE wording.
- `examples/dispatcher-sync.php`: The copied comments use STE wording.
- `examples/worker.php`: The copied comments use STE wording.

The generated headers differ only in the stub hash that the generator writes:

- `crates/php_sys/rapira_arginfo.h`: The generated stub hash records the changed stub comments.
- `crates/php_sys/rapira_exception_arginfo.h`: The generated stub hash records the changed stub comments.
- `crates/php_sys/rapira_http_arginfo.h`: The generated stub hash records the changed stub comments.

## Verification status

The source comparison records each functional, comment-only, and generated stub-hash delta. The approved plan copy has SHA-256 `02F3BB5F4E8BE23E3613AE58D271BF79ACCBB65AF3384CBB07541BED15CCA52B`.

Static validation covers PowerShell parsing, workflow structure, immutable action references, repository whitespace, the instruction file, and the release matrix. The CI workflow runs PHP 8.4 and 8.5 builds and tests on native x64 and ARM64 runners. The release workflow builds PHP and Rapira on the same native architectures and runs a PATH-stripped HTTP smoke test for every archive.

On native ARM64, PHP 8.4.25 and PHP 8.5.10 each passed `cargo test --locked --workspace --target aarch64-pc-windows-msvc` with 416 passed, 0 failed, and 8 ignored. The workspace and e2e Clippy gates pass with warnings denied. The static-file suite also passed all 38 tests in normal parallel mode after its TTL proofs moved to the test-only manual clock. Both PHP release bundles passed the PATH-stripped HTTP smoke test, including the PHP 8.4 shared OPcache path and the PHP 8.5 built-in OPcache path. No x64 binary was built or run on the ARM64 host; x64 executable validation runs on native x64 CI.

The plan describes two fcntl tests, but core has two fcntl assertions in one test. The port retains the bind and accept test. The Release/Acquire comment named in slice 1 is in `crates/php_sys/src/scoreboard.rs`; its Windows wording belongs to slice 3. The Unix-derived port-80 fallback is in config `lib.rs`, and its planned removal is complete. Core `module.c` has no timer platform guard, so the Windows per-job guard remains in the copied request initialization with the plan's widened condition.

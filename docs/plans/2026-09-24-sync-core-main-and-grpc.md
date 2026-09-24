# rapira-windows: sync to core main and port the gRPC plugin

## Context

Upstream `rapira-rs/rapira` has a gRPC feature on branch `feature/grpc-connectrpc` (head `522acad`, 19 commits on top of `main` = `a36a356`). It adds a second plugin, `rapira_grpc`, that serves unary RPCs from PHP over gRPC, gRPC-Web and Connect on one listener, with a new PHP surface (`Rapira\Grpc\*`), a `[grpc]` config table with its own worker pool, a shared accept-loop crate `rapira_net`, and four new test suites.

`rapira-windows` is a slice-by-slice port of upstream (see `UPSTREAM.md`). It was synced from upstream commit `5f26e1d9`, which is ~130 commits behind `a36a356`. The gRPC branch depends structurally on several of those commits (per-plugin pools, the intake `Unit`/`Held` refactor, `HeaderMap` metadata, `receive()` discarding a host-closed unit, the `Tls` namespace move, the api `unary` verb). The user decided:

- **Config and CLI**: adopt upstream's shape: `[http.pool]` + `[grpc.pool]`, config file required, CLI reduced to `rapira serve <CONFIG>`. Breaking, allowed before 1.0. `AGENTS.md` settled design, README, examples and harness follow.
- **Scope**: full sync to upstream `main` (`a36a356`) first, then port the gRPC branch on top. Two stages on the designated branch `claude/admiring-edison-w9d0cf`.
- **Commits**: author and committer `Valery Piashchynski <piashchynski.valery@gmail.com>` with a matching `Signed-off-by` trailer. No AI attribution, no `Co-authored-by`, no session trailers. Conventional Commit subjects. Mechanics: `git -c user.name='Valery Piashchynski' -c user.email='piashchynski.valery@gmail.com' commit -s -m '...'` for every commit (the container identity is not the user's); verify with `git log --format='%an <%ae>%n%b' -1` before each push.
- **Delivery**: push the branch (`git push -u origin claude/admiring-edison-w9d0cf`) when stage 1 is complete so a PR and CI can run, then continue stage 2 on the same branch and push again. Do not open pull requests. Each PR description (drafted in the scratchpad for the user to paste) stays short and records the new dependencies with their reasons, as `AGENTS.md` requires.
- **Windows review-fix deltas**: keep the Host/authority validation in `check.rs` and the early-EOF `read_slice`, and document both in `UPSTREAM.md` (section 1.0).
- **Stored plan**: as the previous sync did, commit this plan as `docs/plans/2026-09-24-sync-core-main-and-grpc.md` in the stage 1 docs commit and reference it from `UPSTREAM.md`.

Windows process model (settled, unchanged): one process, ZTS PHP, static interpreter thread pools, MINIT once before any thread, no fork, no master crate, no Unix sockets, no `libc`, `windows-sys`, console Ctrl+C/Ctrl+Break drains, `TerminateProcess` on forced exits. With two plugins the process runs **one static thread pool per configured plugin table**, sized by `http.pool.processes` and `grpc.pool.processes`.

Reference checkouts on this machine (read-only): upstream `/home/user/rapira-rs/rapira` (fetch refs `origin/main`, `origin/feature/grpc-connectrpc`), PHP contract `/home/user/rapira-rs/contract` (`fe62f7b`), connect-rust `/home/user/rapira-rs/connect-rust` at the pinned rev `0bb21af`.

Verification caveat: this Linux container cannot build the workspace (needs native MSVC, LLVM, and a ZTS PHP devel tree). `cargo` here can only check manifests and lockfile resolution. The authoritative build and test run is the `windows-latest` / ARM64 CI matrix after each push. Plan the commits so each stage is a coherent, CI-green unit.

## Stage 1: sync to core `a36a356`

Goal: the Windows repo mirrors upstream `main` file for file, keeping only the documented Windows deltas, so that stage 2 can follow the gRPC branch diff closely. Record the new source commit in `UPSTREAM.md` and rewrite its delta tables.

Method per file: copy the upstream `origin/main` file, re-apply the Windows delta (read the current Windows file for the exact Windows code; `UPSTREAM.md` lists the documented ones, and section 1.0 lists the undocumented ones), then apply the STE comment wording used across the repo. Classes: (A) portable as is, (B) portable with a Windows delta, (C) Linux/Unix-only, skip, (D) conflicts with the Windows design, skip. Upstream file paths below are `git -C /home/user/rapira-rs/rapira show origin/main:<path>`.

Facts that shape stage 1:

- Every php_sys hunk conflicts mechanically: Windows has no `Job` wrapper (`job.ctx.x` is `job.x`), a crossbeam intake, safe `fn`s in `exchange/*` with no `armed_at`, and STE comments. Take the final upstream file and re-apply the Windows deltas rather than replaying commits.
- Versions stay on the Windows line (0.8.0 for every workspace package, `src/version_tests.rs`); upstream's 0.8.1 bumps are release bookkeeping.
- Windows `Cargo.lock` is already at or above every upstream crate version (bindgen 0.73.2 etc.); the only lock delta is dropping `tracing-attributes` via the workspace `tracing` without default features.
- On ZTS, `php_request_shutdown` frees the thread's virtual cwd and `php_request_startup` re-copies the process cwd (`main/main.c:1970,2402`, `Zend/zend_virtual_cwd.c:233-247`), so a request always starts in the process working directory. Upstream's classic cwd persistence does not exist on Windows.

### 1.0 Undocumented Windows deltas to preserve (from Windows commits `6a17377`, `f57d99c`)

- `crates/plugins/http/src/check.rs`: Host and absolute-form authority validation (`is_valid_host`, `is_valid_authority_host`, `is_valid_ipv_future`) and the relaxed `is_safe_field_name`. Keep, and add a row to `UPSTREAM.md`.
- `crates/api/src/lib.rs` `read_slice` returns `UnexpectedEof` on a short file (upstream truncates). Keep; it moves to the tests crate with `collect` (see 1.3).
- `extension_api::Protocol` was deleted on Windows; upstream keeps `#[non_exhaustive] pub enum Protocol { Http, Grpc }` and inserts `Protocol::Http` in `handler.rs:241` and the static_files test. Restore it (byte-identical `middleware.rs`).
- Windows fixed-length reply drain (`bridge.rs:64-75`, `handler.rs:343`, tests `bridge.rs:466-496`, `handler.rs:715-763`): delete when the upstream watermark lands (1.4).

### 1.1 `crates/scoreboard`

- `538b00d` (A): drop `restarts` from `SharedSlot` and `SlotSnapshot`, `_tail: [u8; 16]`, `Event::Restart` and its `rapira_worker.rs:90` use go with it.
- `52dc11f` (B): `slot(i) -> &'static SharedSlot` (panics out of range); remove the `expect`/`unwrap` at php_sys `start.rs:179,833,857` and `scoreboard.rs:102`.
- `6ef8f0d` (D): keep the `1..=SB_MAX_SLOTS` check in `create` (php_sys passes the configured count; upstream moved the check to the master). `4778798` `slice()` (D, no Windows caller). `1cd460d` doc (B: Windows sentence).

### 1.2 `crates/php_sys` (take the final upstream files, re-apply Windows deltas)

- **Take (A)**: `7a8fd31` HeaderMap end to end (delete `fold.rs`, `FieldLines`; `http = "1"` dep; `joined_field`), `e14b673` folded into it, `0c3039c` no `body_coded`, `6abb8d2` no `server_vars`/`getenv_cb`, `4cc3acf` `context::{ScriptPaths, set_script, script}` and the trimmed `Request` (no `query`, `script_*`, `document_root`), `b663fb3` `Body::Raw(Cursor<Vec<u8>>)`, infallible `ExchangeState::new`, lazy `RequestView`, `93cbb26` boxed intake (`bounded::<Box<Context>>`, no re-box at `start.rs:363,404,432`), `d37727f` cycle unit as `Option<*mut ExchangeState>` (rename away from the Windows `Unit` enum; stage 2 reuses the name for the intake), fci-store and NULL-guard removals, `strip_framing`/`is_hop_by_hop` removal (the front strips hop-by-hop), `61d46c5` no `Work::__destruct` (regenerate `rapira_arginfo.h` and `rapira_http_arginfo.h` from the Windows stubs), `52dc11f` binding trims (`sapi_activate`, `php_output_deactivate`, `php_call_shutdown_functions`, `zend_observer_fcall_end_all`, `php_handle_aborted_connection`, `TRACK_VARS_FILES`), no `sapi_deactivate_cb`, `superglobals` field removed (`!dispatcher`), no `#[allow(clippy::all)]`, `1966b42` log-level early return in `dispatcher.rs`, `try_send` before `blocking_send` in `Context::finish`, borrowing `PendingGuard`, `4301363` `Outcome::Exit`/C `EXIT`, `Verb::Interim`, C-only class-entry globals, second NULL check in `rapira_rs_exchange_drop`, dead `wrapper.h` prototypes (`rapira_sg/eg/cg/pg` family, `rapira_init_call_stack`), `66db4e4` php_sys tokio features `["time"]` only, `d90f9df` factual comment fixes, `b0e4ff6` `PHP_STREAM_FLAG_NO_FCLOSE` check in `rapira_release_temporary_streams` (`module.c:449`), `521d97b`+`53a3615` `receive()` discards a host-closed unfinalized exchange with a debug log.
- **Windows deltas (B)**: `4301363` MINIT: call `php_module_startup(&mut module, &raw mut rapira_module_entry)` directly in `start_pool` but keep `sapi_startup` routed through `rapira_sapi_startup` (priority bridge) and keep `check_linked_php` using bindgen's `PHP_VERSION_ID` in place of the `rapira_headers_php_version_id` shim (drop the shim). `ac1e9bc`/`e51e042`: set `SG(options) |= SAPI_OPTION_NO_CHDIR` in `rapira_thread_init` (SG is per thread under ZTS), no `rapira_child_init`. `1966b42` `SENDFILE_ROOT`: keep the Windows `Mutex` (the Windows `exchange_loop` tests set several roots in one binary); keep the UTF-8 path check and the `seek_read`/verbatim-path checks. Keep the whole thread pool in `start.rs`, `PoolHooks`, `quota.rs` per generation, the timer shims, `ZEND_TLS` statics, the vectorcall bridges, `rapira_dispatcher_thread_init`, `forget_dispatcher`, the lossy `path_bytes`, the timer-armed park in `respond.rs`, `.layout_tests(true)`, the `zend_ce_throwable` workaround in `build.rs`.
- **Skip (D/C)**: `13411c4` (`#error` on ZTS), `187b25d` single-slot scoreboard, `38e485c` `Rapira::shutdown` removal (keep `shutdown() -> bool`; do remove `handle_blocking`, see 1.7), `e51e042` child-init placement, the SIGPIPE/`Pool::watchdog_tick`/`_zend_op`/SIGPROF comment hunks, `build.rs` macOS sysroot hunk.
- **Stubs**: `rapira.stub.php` Mode docblock says `[http.pool] mode` (`4778798`); regenerate all three arginfo headers on Windows (`dev.ps1 -Task stubs`); hashes follow the Windows LF stub bytes.

### 1.3 `crates/api`

- `7a8fd31` (A) `HeaderMap` in `ReplyEvent` and `Request`; `0c3039c` (A) no `body_coded`; `5b71e97` (A) `LISTEN_BACKLOG` private; `d90f9df` (A) `Tls` doc fix; `4f80109` (B) move `Reply::collect`, `read_slice`, `Response` into `crates/tests/src/lib.rs` as `collect(reply)` carrying the Windows `seek_read` + `UnexpectedEof` code and its three tests; restore `Protocol` (1.0). Keep the Windows `prepare.rs` (TCP-only `ListenAddr`, `PreparedListener { listener, addr }`, `into_listener`, no `SO_REUSEADDR`).

### 1.4 `crates/plugins/http` (the delivery watermark replaces the Windows fixed-length drain)

- Take the final upstream `bridge.rs`, `handler.rs`, `request.rs`, `response.rs`, `lib.rs`, and the non-Linux branches of `serve.rs`: `ConnectionState { closed, flushes }` watch, `TimedIo::new(io, timeout, state)` counting flushes with `send_if_modified`, `InflightReqCount.end_flush: OnceLock<u64>`, `framed_length` (HEAD/1xx/204/304 = 0, then Content-Length, then `is_end_stream`, then exact size hint), `RespBody { kind, guard, transport }` countdown, `Drop for ReplyBody` handoff, `spawn_drain` with the noop-waker pre-poll and the close-vs-flush race, bodiless replies through `spawn_drain`, `Serving { shared, graceful, builder }` with `start`/`spawn_conn`/`drain`, non-Linux `Stop(watch::Sender<bool>)`, `is_skipped_accept`, `Shared.cfg: Config`, body reservation from `size_hint().lower()` capped at `max_body_size` (`0ea1180`, `1cd460d`), `Peer` by value, `target` only for absolute/authority forms, hyper default builder calls removed (`770c253`), `response_headers(HeaderMap, Option<u64>)` with the static `HOP_BY_HOP` list plus `Connection`-listed names, `Protocol::Http` inserted, the upstream unit tests (`DrainSource`, `parked_drain`, `flushed_drain_survives_a_later_close`, `drain_cancels_when_the_close_beats_the_flush`, queued/pending rows, `completed_body_without_a_watermark_cancels_php`, `Gate`/`Scripted::parked`/`MapBody`/`serve_raw` tests needing tokio `io-util` in dev-deps, `head_with_a_positive_content_length_completes_at_the_head`, prime-period file-slice test, `shutdown_joins_the_server_after_run_is_cancelled`).
- Windows deltas: `seek_read` in `read_slice`; `create_acceptor` from `PreparedListener::into_listener()`; `is_fatal_accept` keeps the Winsock set (10038, 10022, 10045) and does **not** treat `ErrorKind::Other` as fatal (that arm encodes the Linux rotation failure); the `2580a4e` table test rewritten with WSA codes (10024 not fatal, 10053 skipped); no `accept_linux.rs`, no `#[cfg(target_os = "linux")]` branches, no `spawn_unix`; README describes a process-wide cache and interpreter threads.
- Delete the Windows fixed-length drain and its tests (1.0).

### 1.5 `crates/middleware/static_files`

- `8fdc78f` (A) `is_miss` matches `ErrorKind::NotADirectory` (restores the case Windows dropped with `ENOTDIR`); `5fa49e2` (B) remove `FillGuard`/`Store::filling` (`cache.rs:137-140,223-243,331-338`, assert at `lib.rs:781`) but keep the Windows test clock (`now()`, `advance_past_ttl`); `52dc11f`/`7c00b3c` (A) replace the three directory tests with `directory_routes_fall_through_without_being_cached`; keep the Windows `eligible` hardening and the 1314 symlink gate.

### 1.6 `crates/runtime`, `crates/config`, `src/`

- runtime: `f305306` (A) infallible `register`, `target`/`authority` pass through; `7c6cfb5` (B) drop the non-Unix `compile_error!` and the cached `dispatcher` flag, **keep** the `Send` bound on `ErasedExt`; `54721eb` (A) `normalize_crlf` removed; `4cc3acf` (A in stage 1) `RapiraBackend { rapira, uploads }`, `new(rapira, filename: &Path, opts)` calls `php_sys::set_script` (stage 2 moves that call to `worker_body` for the HTTP pool only); `b663fb3`/`0c3039c`/`6abb8d2`/`7a8fd31` companions (`Cursor` body, no `body_coded`/`server_vars`, `headers.get(CONTENT_TYPE)`/`get_all`); `52dc11f` (A) multipart items `pub(super)`; `66db4e4` (B) tokio without `rt`/`sync`, keep `windows-sys`. Keep `ShutdownWatcher`, `win_ctrl`, `Running::serve`; no `serve_worker`/`sigwait`.
- config (`4778798`, `cca6447`, `bece355`, `d90f9df`): `Settings { http: HttpSettings, supervisor, log }`, `HttpSettings.pool: PoolSettings` from `[http.pool]`, `resolve(path: &Path)` with the file required, `resolve_pool(section, table, config_dir)` with table-prefixed errors and a required entrypoint, `sendfile_root: PathBuf` defaulting to the entrypoint directory (moved out of `main.rs`), `Overrides` and `RunMode: FromStr` removed, `Listen: FromStr<Err = anyhow::Error>` with the Windows TCP-only message, top-level `[pool]` rejected (`deny_unknown_fields`). Keep the Windows pool key set (`entrypoint`, `processes` = interpreter threads, `mode`, `max_requests`). Tests use `std::path::absolute` and a Windows example config.
- `src/main.rs`: CLI `serve <CONFIG>`; `settings.http.pool.*`; boot check that the entrypoint opens and is a regular file (`8626757`, exit 1); `set_current_dir(entrypoint parent)` before PHP boots (`ac1e9bc`, B: the process cwd is what every request starts in on ZTS); infallible `logging::init` (`1714638`) and `register` (`f305306`); keep the pidfile, the sendfile-root canonicalization (now from config), the uploads probe and the spool sweep, `force_exit`. `src/worker.rs`: `effective_quota` drops the wall-clock entropy (`6c89106`, keeps `thread_index`; bounds test holds). `src/logging.rs`: infallible.

### 1.7 `crates/tests`

- `src/lib.rs`: `submit(h, req)` over a `OnceLock` current-thread runtime (`583b444`), `wait_app_record` (`30e17b9`), `app_records -> (Vec<AppRecord>, Vec<String>)` (`8fee29b`), `Response` + `collect` (`4f80109`, with the Windows early-EOF), `req()` calls `set_script` and drops the removed fields, `Resp.trailers: HeaderMap`, capture layer without `tracing-log` (`bf3f44f`). Keep `run_worker` without the `ini` argument, `set_php_env`, `extension_ini`, `PHP_INI_SCAN_DIR`, `seek_read`. `Cargo.toml`: `extension_api`, `http` as deps; dev `http-body-util`, `rapira_http`, `serde_json`; tokio `rt`, `time`; no `tracing-log`, no `libc`, keep `windows-sys`.
- Remove `handle_blocking` everywhere (`38e485c`): every suite goes through `submit`; the php_sys `start.rs` timer tests get a local current-thread runtime. Keep `r.shutdown()` where the verdict matters and `drop(r)` elsewhere.
- Deletions (`420eb89`, `52dc11f`, `61d46c5`, `b663fb3`, `f305306`): `basic_tests` `worker_survives_exit`, `many_producers_test`, the repeated assertion at `:549-553`; `extension_tests` `an_extension_drives_concurrent_requests_through_php`, `duplicate_extension_name_is_rejected` + `Fixed` ext; `general_tests` `in_user_include_flag_reset_between_requests` + `stuck-flag-worker.php`, `post_body_survives_partial_reads` + `Trickle` + `input-worker.php`; `exchange_loop` `explicit_destruct_call_is_a_noop` + the `destruct-explicit` probe; `app_logger_limits.rs` (move `cycles_are_broken_without_a_diagnostic` into `app_logger.rs`, delete `limits-large-array.php`); the harness duplication at `harness.rs:594-630`. Keep the Windows `static_pool_starts_n_threads`.
- New in-process rows: `php_ext_tests` `bcmath_success` with a success-only helper (the other five extensions are outside the Windows profile: D); `server_variables` asserts `SCRIPT_FILENAME`/`DOCUMENT_ROOT`/`SCRIPT_NAME`/`PHP_SELF` (check Windows separator output of `set_script`); `request-worker.php` and `exchange_loop.rs:544`, `ported_tests.rs:509` follow HeaderMap.
- e2e (B, all with `let pid = srv.pid()` since Windows `wait_workers` returns thread indices): `retained_spl_tempfile_survives_next_request` + `retained-temp-worker.php`; `abandoned_exchange_is_discarded_by_next_receive` + `cancelled-receive-worker.php` (log level debug, "discarded an unfinalized exchange"); `a_top_level_pool_table_refuses_to_boot`, `a_missing_entrypoint_refuses_to_boot` + helpers `spawn_boot_failure_with_entrypoint`/`boot_failure`; the seven streaming tests (`fixed_length_completion_does_not_cancel_php`, `empty_fixed_length_…`, `file_completion_…`, `head_completion_…`, `middleware_body_change_preserves_php_finalization` over the Windows prepared TCP listener, `later_connection_error_…`, `reset_during_buffered_write_cancels_php` with socket2 `set_recv_buffer_size`/`set_linger` instead of `libc::setsockopt`; confirm the 32 MiB stall on Windows) + `completed-response-worker.php`, `buffered-download-worker.php`; the merged browscap test if the harness `spawn_with_phprc_and_config` exists (else skip); `render_config` writes `[http.pool]` before `[http]` and the harness runs `serve <toml>`. Skip (C/D): `an_unreadable_entrypoint_refuses_to_boot`, `sequential_requests_go_round_the_pool`, `killed_master_leaves_workers_to_drain`, `classic_mode_keeps_the_working_directory_across_requests` (ZTS resets the cwd per request), `apm`/`reload`/`scaling`/DRBG.

### 1.8 CI, build, docs

- `ci.yml`: `timeout-minutes: 45` on the x64 test job (`50f940f`). `build-binaries.yml`: prune any rolling asset whose name lacks the version (`b69f8b4`); smoke config uses `[http.pool]`. `ci/php-configure-flags.txt`: `--enable-bcmath` (dependency-free; README extension list and `UPSTREAM.md` follow). `.gitignore`: `.DS_Store`. Root `Cargo.toml`: workspace `tracing = { version = "0.1", default-features = false, features = ["std"] }`, every crate takes shared deps from the workspace table (`66db4e4`).
- Docs: README Windows sections for `[http.pool]`, `rapira serve rapira.toml`, the working directory rule ("every request starts in the process working directory, which is the entrypoint directory; a `chdir()` does not persist across requests"), bcmath; `examples/rapira.toml` `[http.pool]` with `dispatcher-sync.php` and `log.level` commented at the default (`c6ee3d5`); `examples/README.md` table; `CONTRIBUTING.md` layout table (config row without "CLI"); `AGENTS.md` settled design (`http.pool.processes` sets the interpreter thread count; `rapira serve <CONFIG>` is the only CLI form); `crates/plugins/http/README.md`; `UPSTREAM.md` rewritten with source commit `a36a356`, the delta tables per crate, and the 1.0 rows.

### 1.9 Stage 1 commit order (each CI-green)

1. `refactor(scoreboard): drop the restart counter and make slot lookup infallible` (1.1 + php_sys call sites).
2. `refactor!: carry headers as HeaderMap and trim the request path` (1.2 A items, 1.3, runtime/http/tests companions for HeaderMap, `Cursor` body, `set_script`, boxed intake, cycle `Option`, `Protocol` restore, stub regen for `Work::__destruct`).
3. `fix(php_sys): keep retained SPL temp streams and discard abandoned exchanges` (`b0e4ff6`, `521d97b`, `53a3615` + their e2e rows).
4. `fix(http): cancel PHP only when a completed response was never flushed` (1.4 + the seven streaming e2e rows; removes the Windows drain).
5. `refactor(static): match a missing directory by error kind and drop the fill guard` (1.5).
6. `refactor(runtime): make register infallible and pass target and authority through` (1.6 runtime + `src/logging.rs`).
7. `feat!: move the pool table under [http] and serve one config file` (1.6 config + `src/`, NO_CHDIR + cwd, entrypoint boot check, jitter, harness `render_config`, the two boot e2e rows, examples, build-binaries smoke config, stub docblock).
8. `test: submit through the production intake and share the record helpers` (1.7 remaining items).
9. `build: enable bcmath in the Windows PHP profile` (+ `bcmath_success`).
10. `ci: bound the x64 test job and simplify the rolling prune` (+ `.gitignore`).
11. `docs: record the sync to core a36a356` (1.8 docs, `UPSTREAM.md`).

### Never port (D), both stages

- `13411c4` (`wrapper.h` `#error` on ZTS, `php -i` detection): Windows is ZTS-only; `build.rs` rejects NTS.
- `187b25d` single-slot `Rapira::scoreboard()`: incompatible with a thread pool; keep the aggregate snapshot.
- `38e485c` removal of `Rapira::shutdown`: Windows `shutdown() -> bool` is the join verdict.
- `7c6cfb5` removal of the `Send` bound on `ErasedExt`: stage 2 serves `Running` values on more than one thread.
- The parts of `4301363` that delete the `sapi_startup` bridge (Windows LOG priority translation).
- `e51e042` `rapira_child_init`, `8626757`'s reliance on the master, `6ef8f0d` cap removal, `4778798` master/pool supervision, all `accept_linux.rs`/EPOLLEXCLUSIVE/eventfd work, `crates/master`, signals, `libc::kill`, Unix sockets, Docker/nfpm/Makefile/`.zed` packaging, the classic cwd persistence test.

## Stage 2: port the gRPC plugin

Source: `origin/feature/grpc-connectrpc` (`522acad`). Follow the branch commit order; each Windows commit maps to one or two upstream commits.

### 2.1 Dependencies (root `Cargo.toml`, `Cargo.lock`)

- Add workspace deps exactly as upstream: `base64 = "0.22"`, `hyper = "1"`, `hyper-util = "0.1"`, `buffa = { version = "0.9.2", default-features = false, features = ["std"] }`, `buffa-descriptor = { version = "0.9.2", features = ["reflect", "json"] }`, `connectrpc`/`connectrpc-health`/`connectrpc-reflection` as git deps pinned to `rev = "0bb21af9736dac7b82065a4252405a8eb65f27ee"` with the same features. The pin is required: `serve_connection` is not in connectrpc 0.9.1 (verified against the tag). connect-rust already carries a `cfg(windows)` arm (WSAEMFILE) and no build scripts; `zstd-sys` compiles through `cc`/MSVC because the health and reflection crates enable connectrpc's default features.
- New members `crates/net`, `crates/plugins/grpc`, both at version `0.8.0` (`src/version_tests.rs` requires one product version; stage 1 keeps the Windows 0.8.0 line).
- Regenerate `Cargo.lock` (`cargo generate-lockfile`/`cargo update -p` on Windows CI-equivalent; here `cargo metadata` can validate resolution). First git source in the Windows lock; `--locked` builds keep working. Record the dependency reasons in the PR description (buffa is the protobuf runtime connect-rust requires; `anthropics/buffa`, pure Rust).
- Add `.gitattributes`: `*.binpb binary`, `*.stub.php text eol=lf`, `*.proto text eol=lf` (gen_stub hashes raw bytes; CRLF checkouts would change the arginfo hash).

### 2.2 `crates/net` (Windows-shaped `rapira_net`)

Mirror upstream `crates/net/src/lib.rs` minus every Unix piece:

- `Stop`/`StopHandle` = the non-Linux `watch` implementation from upstream (`Stop(watch::Sender<bool>)`, `StopHandle = watch::Receiver<bool>`).
- `pub trait Serve { fn spawn_tcp(&self, stream: tokio::net::TcpStream, peer: SocketAddr); }` (no `spawn_unix`).
- `Acceptor::adopt(prepared: PreparedListener, stop, rt)` uses `prepared.into_listener()` + `TcpListener::from_std` inside `rt.enter()`; `run` is the upstream non-Linux `select!` loop (biased stop, `accept`, `set_nodelay(true)`).
- Accept-error rules: move the Windows table from `crates/plugins/http/src/serve.rs` (`is_fatal_accept`: WSAENOTSOCK 10038, WSAEINVAL 10022, WSAEOPNOTSUPP 10045; skip ConnectionAborted/Reset/Interrupted; 100 ms sleep otherwise) and its unit test.
- `ServerThread` verbatim from upstream (`run(name, body)`, `is_running`, `stop`, `shutdown`, private `join_thread`), thread `rapira-{name}`, 2-worker runtime `rapira-{name}-io`.
- `Cargo.toml`: `extension_api`, `anyhow`, `tracing`, `tokio` (`rt`, `rt-multi-thread`, `net`, `time`, `macros`); no `libc`, no `tempfile`.
- HTTP plugin (`crates/plugins/http`): follow upstream `96047cc` + `83e02b7`: delete its private accept loop and thread code, implement `Serve::spawn_tcp`, use `ServerThread` and `PrepareCtx::bind`. Keep the Windows `bridge.rs` deltas (`seek_read`).

### 2.3 `crates/api`

Follow upstream `017bb01` + `6f2f3d2` + `83e02b7` + `e421bbc`: `Backend::exec` gets a default refusal, add `Backend::unary`, `Php::unary`, `UnaryCall`, `RpcProtocol`, `UnaryReply`, `RpcStatus` verbatim. `prepare.rs`: add `impl Display for ListenAddr` and `PrepareCtx::bind(&ListenAddr) -> Result<PreparedListener>` (TCP only). Windows delta replacing upstream's `listener_fds()`: `PrepareCtx { addrs: Vec<ListenAddr> }` records each resolved bind and exposes `pub fn listener_addrs(&self) -> Vec<ListenAddr>`, so the test harness `tcp_addr(&ctx, index)` keeps its signature (the `PreparedListener` itself is moved into the plugin and unreachable). `Addr::Unix` stays declared (PHP union type), never produced.

### 2.4 `crates/config`

Follow `49caad0` + `54cecc2`: new `grpc.rs` (`GrpcSection`, `GrpcSettings`, `resolve_grpc`, the validation rules and tests, minus the `unix:` listen row), `Settings { http: Option<HttpSettings>, grpc: Option<GrpcSettings>, supervisor, log }`, "no plugin configured" error, shared `parse_listen` and `nonzero_timeout`, `grpc.pool.mode` must be `dispatcher`. Windows delta: `Listen::Tcp` only, `grpc.pool` keys are the Windows pool key set.

### 2.5 `crates/php_sys`: PHP surface and the unary call

Follow `eb9f408` (Tls to `\Rapira\Tls`), `5e4c114` (value types), `499fdc9` (`Unit` on the intake + `Held`), `bf93828` (gRPC dispatcher), `14f73c1` (unary call), `1496cb9`, `e421bbc`, `6f2f3d2` (`Front`, `release`, dedupe):

- C: add `rapira_grpc.c`, `rapira_grpc.stub.php`, generated `rapira_grpc_arginfo.h`; `rapira_classes.c/.h` registrations and the two object layouts; `wrapper.h/.c` shims `rapira_symtable_str_find`, `rapira_zval_enum_case`, renamed `rapira_ce_tls`; `allowed_bindings.rs` additions; `build.rs` compiles `rapira_grpc.c` and tracks the three new files; `dev.ps1` clangd `$sources` gains `rapira_grpc.c` (the `stubs` task already globs `*.stub.php`). Regenerate all four arginfo headers with `dev.ps1 -Task stubs` on Windows (cannot run here; the implementer regenerates on the VM/CI, or hand-carries upstream's generated header and re-hashes after the STE wording pass. The hashes must match the LF stub bytes).
- Rust: `exchange/grpc.rs` verbatim (it is platform-neutral; uses `AddrOwned`/`build_address` from `request.rs`), `types.rs` additions (`Mode::GrpcDispatcher { script, services }`, `GrpcService`, `GrpcMethod`, `MethodKind`, `Unit`, `GrpcProtocol`, `GrpcRequest`, `GrpcOutcome`, `GrpcStatus`, `GrpcJob`), `values.rs` constructors and `Metadata` validation, `exchange/mod.rs` `Front` + `Held` + `release`, `receive.rs` per-unit pull, `handler.rs` `call()` over the crossbeam intake carrying `Unit`, `rapira_worker.rs`/`classic_worker.rs` `into_http()`.
- **Windows-only: `rapira_mode` becomes per thread.** In php-src `ZEND_TLS` expands to `static TSRM_TLS` (`__declspec(thread)` on MSVC), so the variable is file-static and no other TU or Rust can read it directly. `rapira_dispatcher.c`: `ZEND_TLS int rapira_mode = RAPIRA_MODE_CLASSIC;` plus `void rapira_mode_set(int mode)` and `int rapira_mode_get(void)` in the same file (lines 32 and 70 keep reading the static). `wrapper.h`: replace `extern int rapira_mode;` with the two prototypes. `module.c:56`: `rapira_mode_get()`. `allowed_bindings.rs`: remove `rapira_mode`, keep `RAPIRA_MODE_*`. `src/lib.rs` extern block: the two functions. `exchange/receive.rs:130`: `rapira_mode_get()`. `start.rs`: delete the process-global write; `worker_main` calls `rapira_mode_set` once per thread after `slot.bind` (`Dispatcher` and `GrpcDispatcher` both map to `RAPIRA_MODE_DISPATCHER`). Any missed reader fails to compile.

### 2.6 `crates/php_sys/src/start.rs`: several pools in one process (Windows-only design)

Today `Rapira::start_pool(mode, processes, hooks)` does module boot and pool start in one call. Split it (the 3-argument `start_pool` stays as a wrapper, so the new method is `add_pool`; Rust forbids two associated items with one name):

```rust
pub struct PoolSpec { pub name: &'static str /* "http" | "grpc" */, pub mode: Mode, pub threads: usize, pub hooks: PoolHooks }
struct Pool { name, intake: Option<Intake>, superglobals, dispatcher, workers: Vec<JoinHandle<()>>, stopping: Arc<AtomicBool>, stop_tx: Option<Sender<()>>, slots: Range<usize> }
pub struct Rapira { pools: Vec<Pool>, board: Scoreboard /* one board */, next_slot: usize, module: Option<PhpModule>, _not_send }
impl Rapira {
    pub fn boot(total_threads: usize) -> anyhow::Result<Self>;          // check_linked_php, Scoreboard::create(total), php_tsrm_startup_ex(total + 1) (a table-size hint), rapira_process_init, sapi_startup, MINIT via sapi_startup_cb; no threads
    pub fn add_pool(&mut self, spec: PoolSpec) -> anyhow::Result<RapiraHandle>; // slots next_slot..+threads (error if the sum exceeds total), pool-local intake bounded::<Unit>(1024), stop bounded(0), stopping, handled, reported_boot_failure, pool-local startup gate; on a spawn error only this pool's parked threads exit, earlier pools keep running, the receiver is unchanged
    pub fn shutdown(self) -> bool;        // every pool's stopping, stop_tx and intake at once; ONE JOIN_GRACE for all threads; join all then drop the module last, else mem::forget(module) and false
    pub fn scoreboard(&self) -> ScoreboardSnapshot;      // whole board, unchanged shape
    pub fn handle(&self) -> RapiraHandle;                // first pool (keeps ~40 test call sites)
    pub fn pool_handle(&self, name: &str) -> Option<RapiraHandle>;
    pub fn start(mode: Mode) -> anyhow::Result<Self>;                                        // boot(1) + one pool
    pub fn start_pool(mode: Mode, processes: usize, hooks: PoolHooks) -> anyhow::Result<Self>; // boot(n) + one pool; name from the mode ("grpc" for GrpcDispatcher, else "http")
}
```

- One scoreboard with a slot range per pool. Per-pool boards would duplicate `SlotSnapshot.id`s and break `prove_per_thread_recycle` (`e2e/lifecycle.rs:893`, keyed by `slot.id`). `slot.bind(global_index)`; `effective_quota` takes the global index.
- Threads `rapira-{name}-{index}`. Log lines become `worker thread {name}/{index} ready|recycling|panicked`; land the format and every consumer in one commit: `e2e/harness.rs:657-668` `ready_threads` returns `BTreeSet<(String, usize)>` parsed from `{pool}/{index}` (the `p.len() == n` predicates at ~18 `wait_workers` call sites keep compiling; the side-by-side test counts one thread per pool), its unit test at `harness.rs:929`, `e2e/timeout.rs:13,41,53` literals, `e2e/lifecycle.rs:814` format string. Keep the literal `"stopping worker threads"` line (`lifecycle.rs:701`). Split the boot log into `booting PHP: {total} threads` and `starting pool {name}: mode {mode:?}, threads {n}`.
- `handled` and `reported_boot_failure` are per pool (created in `add_pool`), so the failboot rule "never served, then unhealthy" is per pool as upstream. The hook closure is process-wide and idempotent: a failboot in either pool exits the whole process 70, which is upstream's master rule too (`events.rs`: "the grpc pool never served, so it failboots"); a pool that already served keeps recycling with backoff as today.
- `worker_main`: `rapira_mode_set` and, for `Mode::GrpcDispatcher { services, .. }`, `exchange::serve_grpc(services.clone())` once before the generation loop (`FRONT`/`SERVICES` are OS-thread-locals that `rapira_thread_init`, `forget_dispatcher` and `cycle_reset` do not touch). `superglobals = !matches!(mode, Dispatcher(_) | GrpcDispatcher { .. })`.
- `Pulled::Job(Unit)` replaces `Box<Context>`; `Unit` already boxes both variants. The per-pool `select_biased!` stop stays.
- `RapiraHandle` unchanged in shape plus upstream `call(GrpcRequest) -> Result<oneshot::Receiver<GrpcOutcome>, HandleError>`; `pending`/`INTAKE_WAIT`/`DispatcherInfo` semantics are already pool-wide on Windows and need no change for N consumers. The single-consumer ordering rows (`a_call_closed_before_the_pull_never_reaches_php`, the `tryReceive()` boot probe) run under `Rapira::start` (one thread) and stay valid; say so in `grpc_calls.rs`.

### 2.7 `crates/runtime`: one `ExtensionRuntime` per pool, one console watcher

- `impl Backend::unary for RapiraBackend` and `refused(HandleError)` from upstream (`14f73c1`, `6f2f3d2`). `run_with_options` stays as upstream (one `Php` per runtime); per-extension backends would need an API upstream lacks.
- Windows delta: `RapiraBackend::new` no longer calls `php_sys::set_script` (a gRPC pool's backend would overwrite the HTTP pool's script paths, which `callbacks.rs` and classic `run_script` read from the process-global `SCRIPT`). `worker_body` calls `php_sys::set_script(entrypoint)` once for the HTTP pool before any thread starts. The tests crate keeps calling `set_script` in `req()` as upstream.
- Working directory with two pools: `set_current_dir` to the HTTP pool's entrypoint directory when an HTTP pool exists, else the gRPC pool's. Document that a gRPC pool script runs in the server working directory and uses `__DIR__` for relative paths (no per-thread cwd code; ZTS resets the cwd per request anyway).
- Console stop fan-out (today `register_stop` does `let _ = slot.set(..)`, so a second `Running::serve` never sees Ctrl+Break): `win_ctrl` replaces `STOP_TX: OnceLock<Sender>` with `Mutex<Vec<watch::Sender<bool>>>`; `register` pushes and replays when `ASKED`; `handler` sends to all; `install` and `uninstall` clear the vector (repeat in-process installs must not keep dead senders); lock with `unwrap_or_else(PoisonError::into_inner)`. Extend `registration_replays_an_early_request` to two senders.
- Keep the `Send` bound on `ErasedExt`; `Running` is `Send` and each one is served on its own scoped thread; `ShutdownWatcher` is `Sync`, so `&watcher` is shared.

### 2.8 `crates/plugins/grpc`

Copy upstream `lib.rs`, `serve.rs`, `dispatch.rs`, `schema.rs`, `README.md`, `Cargo.toml` (product version). Windows deltas: `impl Serve for Serving` has only `spawn_tcp`; `dispatch.rs` stays byte-identical (its `unwrap_or(Addr::Unix(None))` compiles because `Addr::Unix` stays declared; a restructure would cost more than it removes). README drops the fork, SIGHUP/SIGUSR2 reload and unix-socket sentences, says the pool is interpreter threads in the server process, and rewrites the limit as "one interpreter thread serves one call at a time; the calls of one connection queue on the pool intake".

### 2.9 Binary (`src/main.rs`, `src/worker.rs`)

Follow upstream `49caad0`/`6f2f3d2` shapes adapted to threads:

- `serve(args)`: `resolve(&args.config)`, logging, pidfile, then `http_pool(...)` and `grpc_pool(...)` each returning `PoolRun { name, host: ExtensionRuntime, args: PoolArgs { mode, entrypoint, threads, max_requests, http: Option<HttpArgs { uploads: Option<Limits>, sendfile_root }> } }` after `host.register` + `host.prepare_all(&mut prepare_ctx)` (bind before PHP boots, fail fast on a port conflict). `grpc_pool` loads `rapira_grpc::Schema` before PHP boots (bad set or unknown service exits 1), maps `ServiceInfo` to `php_sys::GrpcService`, registers `GrpcServer` with `drain_grace: supervisor.drain_grace()`, keepalive 10 s/10 s.
- `worker::worker_body(pools: Vec<PoolRun>, grace) -> anyhow::Result<WorkerOutcome>`: install the watcher; spool dir only for an HTTP dispatcher pool; `Rapira::boot(sum of threads)` (Err: code 70, joined true); one process-wide `on_boot_failure` closure cloned into every `PoolHooks` (sets `boot_failed`, stops every `Stopper` in an `Arc<OnceLock<Vec<Stopper>>>`); `add_pool` per pool (Err: `let joined = rapira.shutdown()`, code 70, honor `joined`, do not shortcut to `joined: true`); build every `Running` with `run_with_options(handle, entrypoint, opts)` before any serve, set the stopper vector, re-check `boot_failed`; serve all `Running`s in `std::thread::scope` inside `catch_unwind`, joining every scoped handle explicitly; `exit_code(boot_failed, flattened outcomes or None on a panic)`; `rapira.shutdown()`; remove the spool dir only when joined; `WorkerOutcome { code, joined, _shutdown }` keeps the watcher alive through the PHP join.
- CLI: `serve <CONFIG>`; remove `Overrides`, `--processes`, `--mode`, `--listen`, `SCRIPT`.

### 2.10 Tests and fixtures (`crates/tests`)

- `Cargo.toml`: as upstream (`rapira_grpc`, `bytes`, `base64`, `http-body-util`, `hyper` client/http1/http2, `hyper-util` tokio, tokio `net`, `serde_json` and `extension_api` as normal deps); `windows-sys` stays; no `libc`.
- `src/lib.rs`: upstream helpers (`fields`, `case_results`, `assert_case_records`, `dispatcher_record`, `app_results`, `grpc_request`, `call`, `outcome`, `echo_descriptor_set`, `echo_services`) on top of the stage-1 helpers.
- `src/grpc.rs` harness: TCP only. `tcp_addr(&ctx, index)` reads `ctx.listener_addrs()[index]` instead of `BorrowedFd`/`listener_fds`; drop the Unix arm of `Conn::open`; `ListenAddr::Tcp` lets become irrefutable (`-D warnings`).
- Suites: `grpc_server.rs` (drop the "grpc over h2c unix" row and the Unix server), `grpc_schema.rs`, `grpc_values.rs` verbatim; `grpc_dispatcher.rs` becomes **`grpc_calls.rs`** ("dispatcher" contains "patch", which triggers the Windows installer-detection heuristic behind the error-740 renames recorded in `UPSTREAM.md`); its `Addr::Unix(None)` context row is dropped like the other Windows Unix cases; note that its ordering-sensitive rows run under the one-thread `Rapira::start`. Port the new rows in `extension_tests.rs`, `failboot_worker_tests.rs` (Windows scenario-thread pattern), `http_values.rs` + `construct.php` (`Rapira\Tls`). `php_sys/src/exchange/receive.rs` busy message comes from `Front::busy()` (today it hard-codes `Rapira\Http\Exchange`).
- Fixtures: copy `fixtures/grpc/{echo.proto,identity.php,unary-worker.php,values.php}` and the two `.binpb` byte for byte; `e2e/fixtures/grpc/echo-worker.php`. `dev.ps1` gains a `grpc_fixtures` task mirroring the Makefile (`go run github.com/bufbuild/buf/cmd/buf@v1.73.0 build ...`), exempt from the PHP/LLVM/MSVC checks.
- e2e: `harness.rs` gets `spawn_ready`, `exit_of`, `stage_grpc`, `render_grpc`, `spawn_grpc`, `spawn_grpc_with_http`, `spawn_grpc_boot_failure` over `SocketAddr`, writing `[http.pool]`/`[grpc]`/`[grpc.pool]`; `ready_threads` parses the pool-qualified line and `wait_workers` takes a pool name. `e2e/grpc.rs`: the two in-process rows via `run_isolated_test`, `grpc_pool_serves_from_rapira_toml`, `http_and_grpc_pools_run_side_by_side`, `grpc_boot_fails_before_php_starts` (exit 1), `ctrl_break_drains_an_in_flight_grpc_call` (exit 0). `e2e/main.rs` adds `mod grpc;`. `build-binaries.yml` smoke config moves to `[http.pool]`.

### 2.11 Stage 2 commit order (each CI-green, Conventional Commits, signed off)

1. `refactor(php_sys): keep rapira_mode per interpreter thread` (2.5 last bullet; no behavior change).
2. `refactor(php_sys): boot the module once and add pools separately` (2.6: `boot`/`add_pool`/one board/shared shutdown, wrappers kept, pool-qualified log line and all its e2e consumers).
3. `refactor(runtime): fan the console stop out to every runtime` (2.7 `win_ctrl`).
4. `feat(api): add unary RPC calls and listener addresses` (2.3).
5. `refactor(net): share the accept loop and server thread` (2.2, HTTP plugin moved first so the existing HTTP e2e proves the shared crate).
6. `feat(php_sys): add the Rapira\Grpc surface and unary calls` (2.5 minus the mode change: C, stubs/arginfo, `Unit`/`Held`/`Front`, `Mode::GrpcDispatcher`, `serve_grpc`, `RapiraHandle::call`).
7. `feat(runtime): serve unary calls through the backend` (2.7 first bullet).
8. `feat(config): resolve the [grpc] and [grpc.pool] tables` (2.4).
9. `feat(grpc): serve unary calls over gRPC, gRPC-Web and Connect` (2.1 deps/lock/.gitattributes, 2.8, 2.10 harness and in-process suites).
10. `feat!: serve [http] and [grpc] pools from rapira.toml` (2.9, e2e gRPC suite, 2.12 docs, UPSTREAM.md).

### 2.12 Docs

README (Windows differences: gRPC example with PowerShell/curl, one thread pool per plugin, TCP only, a failboot in either pool exits the process with 70, the working directory rule for a gRPC pool), `crates/plugins/grpc/README.md`, `examples/rapira.toml` commented `[grpc]`/`[grpc.pool]` block, `examples/README.md`, `CONTRIBUTING.md` (test placement rule: unit tests only without fixtures, doubles or sockets; harness code under `crates/tests/src/`, fixtures under `crates/tests/fixtures/`; `grpc_fixtures` task), `AGENTS.md` (settled design: one static interpreter thread pool per configured plugin table; `http.pool.processes`/`grpc.pool.processes` set the thread count; the PHP contract at `rapira-rs/contract` comes first for anything PHP can see; test placement rule), `UPSTREAM.md` (stage 2 delta table; source `522acad`).

## Verification

Per stage, on the Windows VM or CI (`windows-latest` x64 and the ARM64 leg, PHP 8.4 and 8.5):

```powershell
.\dev.ps1 -Devel $env:PHP_DEVEL_DIR -Runtime $env:PHP_RUNTIME -Task build
.\dev.ps1 -Devel $env:PHP_DEVEL_DIR -Runtime $env:PHP_RUNTIME -Task test
.\dev.ps1 -Devel $env:PHP_DEVEL_DIR -Runtime $env:PHP_RUNTIME -Task test_e2e
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets --target x86_64-pc-windows-msvc -- -D warnings
cargo clippy --locked -p tests --features e2e --tests --target x86_64-pc-windows-msvc -- -D warnings
```

In this container: `cargo metadata --locked` for manifest and lock validity (build scripts do not run, so this works without PHP); `cargo generate-lockfile` here for the stage 2 git dependency (crates.io is reachable through the proxy; if cargo's libgit2 fetch fails TLS behind the proxy, set `CARGO_NET_GIT_FETCH_WITH_CLI=true`); `git diff` against upstream to prove platform-neutral files are byte-identical; a PowerShell parse check of `dev.ps1` is not possible here (no pwsh), so keep that edit minimal and mirror the existing task structure exactly. Because no Windows build runs here, re-read every ported Rust file for the mechanical Windows deltas (no `Job` wrapper, crossbeam intake, `seek_read`, `windows-sys`) before each commit, and treat the first CI run after each push as the compile check: fix and re-push until the matrix is green.

Proof of mirror after stage 1: for every file `UPSTREAM.md` calls byte-identical, `diff <(git -C /home/user/rapira-rs/rapira show origin/main:PATH) PATH` is empty; for every delta file the diff shows only the documented Windows hunks. Same check against `origin/feature/grpc-connectrpc` after stage 2.

Targeted proofs beyond the ported suites:

- Stage 1: every ported lifecycle fix has its upstream test (completed-response, cancelled-receive, buffered-download, retained-temp fixtures) passing on Windows; the deleted Windows fixed-length drain is covered by `fixed_length_completion_does_not_cancel_php` and `later_connection_error_does_not_cancel_completed_response`; `a_top_level_pool_table_refuses_to_boot` proves the config cut-over; the `server_variables` script-path assertions prove `set_script` on Windows paths.
- Stage 2: `http_and_grpc_pools_run_side_by_side` proves two pools with different modes in one process (`get_mode()` per thread); the boot-failure e2e proves exit 70 from the gRPC pool with a healthy HTTP pool; `ctrl_break_drains_an_in_flight_grpc_call` proves the fan-out stop; `grpc_boot_fails_before_php_starts` proves schema validation precedes PHP boot; `a_silent_peer_loses_its_connection` and the drain rows prove the shared `ServerThread` on Windows.

## Out of scope

- Streaming call kinds (the contract declares them; upstream serves unary only).
- Unix sockets, TLS termination, reload.
- Docs site pages (`rapira-rs.github.io`).

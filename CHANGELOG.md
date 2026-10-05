# Changelog

## [0.9.0](https://github.com/rapira-rs/rapira-windows/compare/v0.8.0...v0.9.0) (2026-10-05)

### Core

- **Plugin pools**: HTTP and gRPC have separate fixed interpreter thread pools under `[http.pool]` and `[grpc.pool]`. Each pool shares one work queue, and `processes` sets its thread count ([core #104](https://github.com/rapira-rs/rapira/issues/104), [core #149](https://github.com/rapira-rs/rapira/issues/149)).
- **Server command**: Run `rapira serve <CONFIG>` in the foreground. Ctrl+C or Ctrl+Break starts a bounded shutdown of plugins and PHP interpreters ([Windows a27e362](https://github.com/rapira-rs/rapira-windows/commit/a27e362d78080c21f1c5d2fe2b7821578e6ebedc)).
- **Boot environment**: Worker and dispatcher boot populate `$_SERVER` with environment values, script paths, `argv`, and `argc`. PHP 8.4 and 8.5 provide boot arguments even with `register_argc_argv` disabled ([core #129](https://github.com/rapira-rs/rapira/issues/129)).
- **Boot recovery**: Failed boot retries without client traffic. A ready interpreter keeps its pool available while another interpreter retries.
- **Plugin API**: `rapira_sapi` owns the shared PHP runtime. Plugins own their PHP classes and configuration, with fewer registration and transport wrappers ([core #144](https://github.com/rapira-rs/rapira/pull/144), [core #145](https://github.com/rapira-rs/rapira/pull/145), [core #194](https://github.com/rapira-rs/rapira/pull/194)).
- **Temporary streams**: Retained `SplTempFileObject` streams remain usable across worker requests ([core #111](https://github.com/rapira-rs/rapira/pull/111)).

### HTTP plugin

- **Request handling**: Reduced request copies and simplified multipart parsing ([core #127](https://github.com/rapira-rs/rapira/pull/127), [core #194](https://github.com/rapira-rs/rapira/pull/194)).
- **Classic working directory**: A classic interpreter retains its virtual working directory across requests. A `chdir()` remains in effect until that interpreter restarts ([core #127](https://github.com/rapira-rs/rapira/pull/127)).
- **Exchange completion**: A complete response does not report false cancellation. Dispatchers can accept new work after client cancellation ([core #113](https://github.com/rapira-rs/rapira/pull/113)).

### gRPC plugin

- **Unary RPCs**: PHP handlers support gRPC, binary gRPC-Web, and Connect with protobuf or JSON messages. The plugin includes health checks and optional server reflection ([core #132](https://github.com/rapira-rs/rapira/issues/132)).
- **Authentication**: Ordered interceptors include a bearer-token check before calls reach PHP. Application calls and reflection require a configured token; health checks remain available without one ([core #36](https://github.com/rapira-rs/rapira/issues/36)).
- **HTTP/2 keepalive**: `[grpc]` supports `keepalive_interval_secs` and `keepalive_timeout_secs` ([core #148](https://github.com/rapira-rs/rapira/pull/148)).

### Observability

- **Prometheus metrics**: `/metrics` reports interpreter states, requests, shared queues, script restarts, and build versions. Counters survive interpreter recycling, and each pool queue is counted once ([core #83](https://github.com/rapira-rs/rapira/issues/83)).
- **Health probes**: `/livez` reports server liveness. `/readyz` requires a ready interpreter in every PHP pool ([core #49](https://github.com/rapira-rs/rapira/issues/49)).
- **Log timestamps**: JSON logs include microseconds in `timestamp` ([core #194](https://github.com/rapira-rs/rapira/pull/194)).

### PHP packages

- **Native runtimes**: x64 and ARM64 packages include matching ZTS PHP 8.4 or 8.5 runtimes built from official source ([Windows a27e362](https://github.com/rapira-rs/rapira-windows/commit/a27e362d78080c21f1c5d2fe2b7821578e6ebedc)).
- **Extensions**: Release builds include `pdo_pgsql`, `pgsql`, `bcmath`, `intl`, `igbinary`, and `redis` ([core #103](https://github.com/rapira-rs/rapira/issues/103), [Windows cb4101e](https://github.com/rapira-rs/rapira-windows/commit/cb4101e69b169defd2fc878045755d2a09d25347)).
- **OPcache**: PHP 8.4 and 8.5 packages include OPcache with JIT disabled. Interpreter threads share the server process's cache ([Windows a27e362](https://github.com/rapira-rs/rapira-windows/commit/a27e362d78080c21f1c5d2fe2b7821578e6ebedc)).

## [0.8.0](https://github.com/rapira-rs/rapira-windows/compare/v0.1.0...v0.8.0) (2026-09-04)


### Features

* sync the Windows server with Rapira core ([fdc10a5](https://github.com/rapira-rs/rapira-windows/commit/fdc10a5636cdcbd375d6ccd31925f9b6922bc462))
* sync the Windows server with Rapira core ([d0180f4](https://github.com/rapira-rs/rapira-windows/commit/d0180f4e4be260c45306514ac65348d1309f827f))


### Bug Fixes

* clarify listen address requirements ([383e270](https://github.com/rapira-rs/rapira-windows/commit/383e270a280989f1a592f9ef2f9076fcba9e0978))
* complete PR review fixes ([6a17377](https://github.com/rapira-rs/rapira-windows/commit/6a1737748b60f48f8885e0b6d1cd9500e6dcdc8d))
* initialize CI paths on the runner ([f1897bc](https://github.com/rapira-rs/rapira-windows/commit/f1897bc673e6ddd09638ca85b7ded5c012e8512f))
* resolve review findings ([3d56798](https://github.com/rapira-rs/rapira-windows/commit/3d567987a59799678ae908b4527c2d3feac4930d))


### Miscellaneous Chores

* bump package version to 0.8.0 ([b8e2acc](https://github.com/rapira-rs/rapira-windows/commit/b8e2acc66daf7c85be16f9a3600545ec828b2e3a))

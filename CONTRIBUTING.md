# Contributing to Rapira for Windows

This repository contains the Windows server, the PHP SAPI, and the HTTP and gRPC plugins. Each plugin has a fixed interpreter thread pool. General product documentation lives in [rapira-rs/rapira-rs.github.io](https://github.com/rapira-rs/rapira-rs.github.io). Keep Windows-specific documentation in this repository.

## Prerequisites

- Windows 10, Windows 11, or Windows Server on x64, or Windows 11 on ARM64
- Native PowerShell 7
- Visual Studio C++ Build Tools with the native x64 or ARM64 MSVC toolset and a Windows SDK
- Native LLVM with `clang.exe` and `libclang.dll`
- Native CMake for ARM64 dependency builds
- Rust stable; `rust-toolchain.toml` selects the stable channel
- Network access to the official PHP 8.4 or 8.5 release source
- The [latest supported Microsoft Visual C++ Redistributable](https://learn.microsoft.com/en-us/cpp/windows/latest-supported-vc-redist) for the native architecture

Use only tools and PHP files that match the host architecture. CI builds x64 on `windows-latest` and ARM64 on `windows-11-vs2026-arm`. Do not cross-build or run binaries for the other architecture locally.

## Build

Run all commands in this section in the same native PowerShell 7 session. Resolve an exact PHP release and build its native ZTS runtime and development tree from verified source. `ci/build-php.ps1` accepts only an install directory below the runner or process temporary directory and exports the paths required by Rapira.

```powershell
$php = .\ci\resolve-php.ps1 -Branch 8.5 | ConvertFrom-Json
$architecture = [Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString().ToLowerInvariant()
$tempRoot = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { [IO.Path]::GetTempPath() }
$install = Join-Path $tempRoot "rapira-php-$($php.php_version)-$architecture"
.\ci\build-php.ps1 -PhpVersion $php.php_version -SourceUrl $php.source_url -SourceSha256 $php.source_sha256 -InstallDirectory $install
.\dev.ps1 -Devel $env:PHP_DEVEL_DIR -Runtime $env:PHP_RUNTIME -Task build
```

The PHP helper verifies source hashes, native tools and output binaries, ZTS headers, PHP identity, and required modules. It reuses a valid install at the same path. The profile is in `ci/php-configure-flags.txt`. x64 uses pinned PHP SDK dependencies. ARM64 builds its dependencies from pinned source with native tools, including a native Perl build. Pass `-Llvm` to `dev.ps1` when LLVM is outside `C:\Program Files\LLVM\bin`.

ARM64 dependencies have a separate cache below the temporary directory. Both PHP versions use the same dependency profile. CI saves this cache before PHP compilation, so a PHP build failure retains the completed dependencies. The source manifest and dependency build scripts determine the cache key.

## Tests

Run the workspace unit tests and the binary-driven end-to-end tests:

```powershell
.\dev.ps1 -Devel $env:PHP_DEVEL_DIR -Runtime $env:PHP_RUNTIME -Task test
.\dev.ps1 -Devel $env:PHP_DEVEL_DIR -Runtime $env:PHP_RUNTIME -Task test_e2e
```

- `test` runs `cargo test --locked --workspace`.
- `test_e2e` builds `rapira.exe` and runs the end-to-end suite with one Rust test thread.
- `coverage` runs `cargo llvm-cov` and writes `lcov.info`. Install `cargo-llvm-cov` and the `llvm-tools-preview` Rust component first.
- `stubs` regenerates every `*_arginfo.h` file from its `.stub.php` source. Pass `-Runtime` and `-PhpSrc` with a matching php-src checkout. Do not edit a generated header directly.
- `grpc_fixtures` rebuilds the protobuf descriptor sets. It requires native Go and uses the pinned Buf version.
- `-Release` selects optimized builds. `-TestFilter` selects tests by name. `-NoRun` builds the tests without executing them.

Use a separate `CARGO_TARGET_DIR` for each PHP minor. The PHP build helper sets `RAPIRA_REQUIRE_EXTS`, which makes a test fail if a required extension is missing.

Run every test task with PHP 8.4 and PHP 8.5 on your native architecture before release-sensitive changes. CI runs both PHP versions on native x64 and ARM64 hosts.

## Lint and format

Run these commands in a PowerShell environment configured by `dev.ps1` or with the equivalent PHP and LLVM variables:

```powershell
cargo fmt --all --check
.\dev.ps1 -Devel $env:PHP_DEVEL_DIR -Runtime $env:PHP_RUNTIME -Task clippy
```

C sources in `crates/sapi` and `crates/plugins` follow `.clang-format`.

Generate the clangd compilation database after you select a native PHP development tree:

```powershell
.\dev.ps1 -Devel $env:PHP_DEVEL_DIR -Task clangd
```

clangd reads the generated commands from the ignored `target/clangd` directory. The task uses native MSVC and PHP headers and does not require a PHP runtime.

## Repository layout

| Path | Contents |
| --- | --- |
| `src/` | CLI, configuration boot, logging, pidfile, and interpreter pool startup |
| `crates/sapi` | PHP SAPI, interpreter pools, C glue, bindings, request loops, and base PHP stubs |
| `crates/php_build` | Native PHP discovery, C compilation, and linking |
| `crates/config` | Shared configuration types |
| `crates/net` | Prepared TCP listeners |
| `crates/scoreboard` | Per-thread states and counters, and shared pool queue counters |
| `crates/observability` | TCP metrics and readiness probes on a PHP-free thread |
| `crates/interceptors/auth` | gRPC bearer-token authentication |
| `crates/plugins/http` | HTTP server and PHP API |
| `crates/plugins/grpc` | Unary gRPC, gRPC-Web, Connect, and PHP API |
| `crates/middleware` | Built-in HTTP middleware |
| `crates/tests` | Binary-driven tests, clients, and fixtures |

## Pull requests

Sign off each commit with `git commit -s`. Fill in the pull request template. Use the [issue forms](https://github.com/rapira-rs/rapira-windows/issues/new/choose) for defects and feature requests. Use [discussions](https://github.com/rapira-rs/rapira-windows/discussions) for questions.

Pull request titles follow [Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/). Use `feat:` for a minor release, `fix:` for a patch release, and `!` or a `BREAKING CHANGE:` footer for a breaking change. Put the issue reference at the end, for example `fix: disarm the timer before interpreter recycle (#84)`.

## Releases

Release Please uses the core Rust workspace configuration. It updates the root package, workspace crates, lockfile, changelog, and release manifest together. The manifest records the last Windows release.

Release Please updates the release pull request after each merge to `main`. Merging that pull request creates the tag and starts four native Windows builds: PHP 8.4 and 8.5 on x64 and ARM64. Each build creates the matching PHP runtime from official release source before it builds Rapira. The release stays in draft state until all four archives and the two architecture checksum files are ready. Re-run failed jobs when a release pipeline fails.

If Release Please creates a tag but the binary workflow does not start, run `build-binaries.yml` with that tag as `ref` and `upload_to`. Set `version` to the tag without `v`. Publish the draft after all assets upload.

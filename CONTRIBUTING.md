# Contributing to Rapira for Windows

This repository contains the Windows server: the PHP SAPI in `crates/php_sys`, the extension runtime, the HTTP front, the fixed interpreter thread pool, and the `rapira` binary. General product documentation lives in [rapira-rs/rapira-rs.github.io](https://github.com/rapira-rs/rapira-rs.github.io). Keep Windows-specific documentation in this repository.

## Prerequisites

- Windows 10, Windows 11, or Windows Server on x64, or Windows 11 on ARM64
- Native PowerShell 7
- Visual Studio C++ Build Tools with the native x64 or ARM64 MSVC toolset and a Windows SDK
- Native LLVM with `clang.exe` and `libclang.dll`
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

The PHP helper verifies the source SHA-256, native build tools, native output binaries, ZTS headers, PHP identity, and required modules. It reuses a valid install at the same path. The dependency-free profile provides fileinfo and mbstring as shared extensions and includes OPcache with JIT disabled. Pass `-Llvm` to `dev.ps1` when LLVM is outside `C:\Program Files\LLVM\bin`.

## Tests

Run the in-process suite and the end-to-end suite as separate tasks:

```powershell
.\dev.ps1 -Devel $env:PHP_DEVEL_DIR -Runtime $env:PHP_RUNTIME -Task test
.\dev.ps1 -Devel $env:PHP_DEVEL_DIR -Runtime $env:PHP_RUNTIME -Task test_e2e
```

- `test` runs `cargo test --locked --workspace`.
- `test_e2e` builds `rapira.exe` and runs the end-to-end suite with one Rust test thread.
- `coverage` runs `cargo llvm-cov` and writes `lcov.info`. Install `cargo-llvm-cov` and the `llvm-tools-preview` Rust component first.
- `stubs` regenerates every `*_arginfo.h` file from its `.stub.php` source. Pass `-Runtime` and `-PhpSrc` with a matching php-src checkout. Do not edit a generated header directly.

Run every test task with PHP 8.4 and PHP 8.5 on your native architecture before release-sensitive changes. CI runs both PHP versions on native x64 and ARM64 hosts.

## Lint and format

Run these commands in a PowerShell environment configured by `dev.ps1` or with the equivalent PHP and LLVM variables:

```powershell
$target = @{ X64 = 'x86_64-pc-windows-msvc'; Arm64 = 'aarch64-pc-windows-msvc' }[[Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()]
if (-not $target) { throw 'Unsupported native Windows architecture.' }
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets --target $target -- -D warnings
cargo clippy --locked -p tests --features e2e --tests --target $target -- -D warnings
```

C sources in `crates/php_sys` follow `.clang-format`.

Generate the clangd compilation database after you select a native PHP development tree:

```powershell
.\dev.ps1 -Devel $env:PHP_DEVEL_DIR -Task clangd
```

clangd reads the generated commands from the ignored `target/clangd` directory. The task uses native MSVC and PHP headers and does not require a PHP runtime.

## Repository layout

| Path | Contents |
| --- | --- |
| `src/` | CLI, configuration boot, logging, pidfile, and interpreter pool startup |
| `crates/php_sys` | PHP SAPI, C glue, bindgen bindings, request loops, and PHP stubs |
| `crates/runtime` | Extension runtime and Windows console control handling |
| `crates/config` | `rapira.toml` and CLI configuration |
| `crates/api` | Native extension contract |
| `crates/scoreboard` | Per-thread counters |
| `crates/plugins/http` | HTTP front |
| `crates/middleware` | Built-in HTTP middleware |
| `crates/tests` | Integration and end-to-end suites |

## Pull requests

Sign off each commit with `git commit -s`. Fill in the pull request template. Use the [issue forms](https://github.com/rapira-rs/rapira-windows/issues/new/choose) for defects and feature requests. Use [discussions](https://github.com/rapira-rs/rapira-windows/discussions) for questions.

Pull request titles follow [Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/). Use `feat:` for a minor release, `fix:` for a patch release, and `!` or a `BREAKING CHANGE:` footer for a breaking change. Put the issue reference at the end, for example `fix: disarm the timer before interpreter recycle (#84)`.

## Releases

Release Please updates the release pull request after each merge to `main`. Merging that pull request creates the tag and starts four native Windows builds: PHP 8.4 and 8.5 on x64 and ARM64. Each build creates the matching PHP runtime from official release source before it builds Rapira. The release stays in draft state until all four archives and the two architecture checksum files are ready. Re-run failed jobs when a release pipeline fails.

After a stable release is published, the `Mirror release to upstream` workflow copies the six Windows assets to the release with the same tag in [rapira-rs/rapira](https://github.com/rapira-rs/rapira). The workflow uses the `UPSTREAM_RELEASE_TOKEN` repository secret. This secret is a fine-grained personal access token with `Contents: Read and write` permission on `rapira-rs/rapira` only. The job fails when the upstream tag or release does not exist. In that case dispatch the workflow by hand with the tag name after the upstream release exists. A prerelease version with a hyphen does not get a copy.

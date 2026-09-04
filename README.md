# Rapira for Windows

[![CI](https://github.com/rapira-rs/rapira-windows/actions/workflows/ci.yml/badge.svg)](https://github.com/rapira-rs/rapira-windows/actions/workflows/ci.yml) [![codecov](https://codecov.io/gh/rapira-rs/rapira-windows/graph/badge.svg)](https://app.codecov.io/gh/rapira-rs/rapira-windows) [![Release](https://img.shields.io/github/v/release/rapira-rs/rapira-windows)](https://github.com/rapira-rs/rapira-windows/releases)

This repository provides the Windows build of [Rapira](https://github.com/rapira-rs/rapira). See the [Rapira documentation](https://rapira.rs/docs/intro/) for shared behavior, modes, configuration, and PHP APIs. This README lists the Windows differences.

## Platform and release packages

- Rapira for Windows supports Windows 10, Windows 11, and Windows Server on x64. It supports Windows 11 on ARM64.
- It embeds ZTS PHP 8.4 or 8.5. The main Rapira build embeds NTS PHP.
- It produces `rapira.exe` and requires the [Microsoft Visual C++ Redistributable](https://learn.microsoft.com/en-us/cpp/windows/latest-supported-vc-redist) for the target architecture.
- Release archives use the name `rapira-v<VERSION>-php<8.4|8.5>-windows-<x86_64|arm64>.zip`. Each architecture has a `rapira-v<VERSION>-windows-<x86_64|arm64>-SHA256SUMS.txt` file.
- Each archive contains `rapira.exe`, the matching project-built PHP runtime, extension DLLs, `php.ini`, `PHP_VERSION.txt`, `README.md`, `LICENSE`, and `PHP-LICENSE.txt`.
- The bundled PHP profile provides fileinfo and mbstring as extension DLLs. It includes OPcache and disables JIT.
- The bundled PHP profile excludes OpenSSL, cURL, SQLite, PDO SQLite, XML, libxml, and iconv. FTP has no TLS support. mbregex is disabled.
- Extension DLLs must match the bundled PHP minor, ZTS setting, architecture, and toolchain. See the [PHP Windows extension requirements](https://www.php.net/manual/en/install.pecl.windows.php).

Download the archive for your PHP minor and architecture from [GitHub Releases](https://github.com/rapira-rs/rapira-windows/releases). Extract the archive. Run `rapira.exe` from that directory.

## Process model and control

- Rapira starts one server process with a static pool of PHP interpreter threads.
- MINIT runs once before the interpreter threads start.
- `pool.processes` and `--processes` set the interpreter thread count.
- The Windows build supports only a static pool. It rejects the main build's scaling settings. It does not support reload or status requests.
- `pool.max_requests` rebuilds an interpreter on the same thread.
- `getmypid()` returns the same process ID in every interpreter.
- A native crash in one interpreter thread stops the server process.
- `--listen` and `http.listen` accept TCP addresses. They do not accept Unix socket paths.
- The first Ctrl+C or Ctrl+Break event drains active work. A second event forces exit code 130.
- Closing the console window does not start a drain.
- A forced exit can leave the pidfile. Remove a stale pidfile before the next start.
- Rapira does not register with Windows Service Control Manager. Use [WinSW](https://github.com/winsw/winsw) when you need a Windows service.

## Windows file and socket behavior

- The static middleware rejects decoded paths that contain a backslash or colon and any path segment that contains a tilde followed by a digit. It ignores trailing dots and spaces when it checks the file name against `forbid`.
- With the [default Windows socket rules](https://learn.microsoft.com/en-us/windows/win32/winsock/using-so-reuseaddr-and-so-exclusiveaddruse), another process under the same user account can bind a specific address on a port where Rapira listens on a wildcard address. Bind Rapira to a specific production address.

## Source builds

Source builds require native PowerShell 7, native MSVC tools with a Windows SDK, native LLVM with `libclang.dll`, and Rust. Use tools and PHP files for the host architecture. Release jobs build PHP 8.4 and 8.5 ZTS from verified official source on native x64 and ARM64 runners.

Generate clangd commands with `.\dev.ps1 -Devel <native-devel-directory> -Task clangd`. The `.clangd` file reads them from the ignored `target/clangd` directory. See [CONTRIBUTING.md](CONTRIBUTING.md) for all Windows build commands.

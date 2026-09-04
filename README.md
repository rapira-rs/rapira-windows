# Rapira for Windows

[![CI](https://github.com/rapira-rs/rapira-windows/actions/workflows/ci.yml/badge.svg)](https://github.com/rapira-rs/rapira-windows/actions/workflows/ci.yml) [![codecov](https://codecov.io/gh/rapira-rs/rapira-windows/graph/badge.svg)](https://app.codecov.io/gh/rapira-rs/rapira-windows) [![Release](https://img.shields.io/github/v/release/rapira-rs/rapira-windows)](https://github.com/rapira-rs/rapira-windows/releases) [![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Rapira is a PHP application server written in Rust. It embeds ZTS PHP through the embed SAPI and serves HTTP through its bundled Hyper front. There is no FastCGI connection between the server and PHP.

Rapira for Windows runs one server process with a static pool of PHP interpreter threads. It supports classic, worker, and dispatcher modes. The produced binary is `rapira.exe`.

## Requirements

- Windows 10, Windows 11, or Windows Server on x64 (`x86_64-pc-windows-msvc`), or Windows 11 on ARM64 (`aarch64-pc-windows-msvc`)
- The [latest supported Microsoft Visual C++ Redistributable](https://learn.microsoft.com/en-us/cpp/windows/latest-supported-vc-redist) for the target architecture

Release assets use the name `rapira-v<VERSION>-php<8.4|8.5>-windows-<x86_64|arm64>.zip`. Each architecture also has a `rapira-v<VERSION>-windows-<x86_64|arm64>-SHA256SUMS.txt` file. Each archive contains `rapira.exe`, a matching project-built ZTS PHP runtime, its extension DLLs, `php.ini`, `PHP_VERSION.txt`, the README, `LICENSE`, and `PHP-LICENSE.txt`. Download the archive for your PHP minor and architecture from [GitHub Releases](https://github.com/rapira-rs/rapira-windows/releases), extract it, and run `rapira.exe` from that directory.

The bundled PHP profile provides fileinfo and mbstring as extension DLLs and includes OPcache with JIT disabled. It omits OpenSSL, cURL, SQLite and PDO SQLite, XML and libxml, and iconv. FTP has no TLS support, and mbregex is disabled. Applications can add extension DLLs built for the exact bundled PHP version, architecture, and toolchain.

Source builds require native MSVC tools with a Windows SDK, native LLVM with `libclang.dll`, and Rust. Release jobs build PHP 8.4 and 8.5 ZTS from official source with `ci/build-php.ps1` on native x64 and ARM64 runners. Test CI uses verified official PHP packages on x64 and builds PHP from source on ARM64.

Local builds must use tools and PHP files for the host architecture. Generate clangd commands with `.\dev.ps1 -Devel <native-devel-directory> -Task clangd`; `.clangd` reads them from the ignored `target/clangd` directory. See [CONTRIBUTING.md](CONTRIBUTING.md) for commands.

## A taste

```php
<?php
// worker.php is booted once in each interpreter thread.
require __DIR__ . '/vendor/autoload.php';

$app = new App();

$handler = static function () use ($app): void {
    echo $app->handle($_SERVER['REQUEST_URI']);
};

while (\Rapira\handle_request($handler)) {
}
```

```powershell
.\rapira.exe serve --mode worker --processes 8 worker.php
curl.exe http://127.0.0.1:8000/
```

Front controller applications use classic mode:

```powershell
.\rapira.exe serve --mode classic public\index.php
```

Dispatcher mode is the default. It gives a resident script direct access to request and response objects. See [examples](examples/README.md) for all three modes.

## Command line

```text
rapira serve [OPTIONS] [SCRIPT]
```

| Option | Default | Description |
| --- | --- | --- |
| `--config <PATH>` | none | Load settings from a `rapira.toml`. |
| `--listen <ADDR>` | `127.0.0.1:8000` | Listen on `host:port` or `:port`. |
| `--processes <N>` | CPU count | Set the number of PHP interpreter threads in the process. |
| `--mode <MODE>` | `dispatcher` | Use `classic`, `worker`, or `dispatcher`. |
| `SCRIPT` | `pool.entrypoint` | Set the PHP entry script. |

CLI values override configuration file values. Unknown configuration keys are rejected.

## Configuration

```toml
[http]
listen = "127.0.0.1:8000"
server_name = "localhost"
server_port = 8000
max_body_size_mb = 8
middleware = ["static"]

[http.static]
root = "public"
forbid = [".php"]

[pool]
entrypoint = "app\\dispatcher.php"
mode = "dispatcher"
processes = 8
max_requests = 0

[supervisor]
pidfile = "rapira.pid"
process_control_timeout_secs = 30

[log]
level = "info"
format = "plain"
```

Relative paths resolve from the directory that contains the configuration file. See [examples/rapira.toml](examples/rapira.toml) for every setting.

The static middleware serves a file before PHP and lets a miss fall through to PHP. The `forbid` list is a case-insensitive file name suffix filter. It is not an access control boundary for alternate file names on volumes with 8.3 names enabled. Disable 8.3 name creation for a document root volume with `fsutil 8dot3name set <volume> 1`, or keep sensitive files outside the document root.

## Logging

The default log level is `error`. Set `[log]` values in `rapira.toml`, or set `RUST_LOG` to replace the complete target filter for one run:

```powershell
$env:RUST_LOG = 'rapira=info,php=warn'
.\rapira.exe serve --config .\rapira.toml
```

## Windows process behavior

- One process owns all PHP interpreter threads. `getmypid()` returns the same value in every interpreter.
- A crash in one thread ends the server process.
- `pool.processes` sets the fixed interpreter thread count. Rapira does not reload or change the pool size while it runs.
- `max_execution_time` is checked at a PHP VM opcode boundary. It does not interrupt a blocked C call.
- `Dispatcher::receive()` outside a fiber blocks one interpreter thread until work arrives or the receive ends.
- The first Ctrl+C or Ctrl+Break drains active work. A second event forces exit code 130.
- Closing the console window does not start a drain.
- A forced exit can leave the pidfile behind. Remove a stale pidfile before the next start.
- The default Windows socket rules let another process running as the same user bind a specific address under a wildcard listener, or a wildcard address under a specific listener, on the same port. Bind a specific production address.

Rapira runs in the foreground. Use a service wrapper such as [NSSM](https://nssm.cc/) or [WinSW](https://github.com/winsw/winsw) when Windows Service Control Manager integration is required.

## Contributing

Build, test, and pull request instructions are in [CONTRIBUTING.md](CONTRIBUTING.md).

## License

[MIT](LICENSE)

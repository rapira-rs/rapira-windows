# rapira-windows

A deliberately minimal, single-process PHP application server for Windows. It embeds
PHP (ZTS) via the embed SAPI and runs one pipeline:

```
pingora (HTTP/TCP) -> tokio mpsc channel -> PHP worker threads (ZTS) -> response
```

Worker mode only. One process, a pool of resident PHP worker threads, no master/fork,
no auto-scaling. The produced binary is `rapira.exe`.

## Requirements

- Windows 10/11 x64 (`x86_64-pc-windows-msvc`).
- **Visual Studio 2022 Build Tools** — the MSVC toolchain + Windows SDK (the linker,
  and the C compiler that builds `wrapper.c`/`module.c` against the PHP headers).
- **LLVM** for `libclang` (bindgen). Install to `C:\Program Files\LLVM`.
- **Rust** stable.
- A **ZTS (thread-safe) PHP 8.4 or 8.5 build** — both the *devel pack* (headers +
  `php8ts.lib`) and the matching *binary zip* (`php8ts.dll`). Get them from
  <https://windows.php.net/download/> (pick the **Thread Safe** x64 build).

## PHP setup

Extract the devel pack and the binary zip, then point the build at them (PowerShell):

```powershell
$env:PHP_DEVEL_DIR = "C:\php\php-8.4-devel-vs17-x64"   # devel pack root (has \lib\php8ts.lib)
$env:RUSTFLAGS     = "-L native=$env:PHP_DEVEL_DIR\lib" # linker finds php8ts.lib
$env:LIBCLANG_PATH = "C:\Program Files\LLVM\bin"        # bindgen finds libclang
$env:PATH          = "C:\php\php-8.4-ts-x64;$env:PATH"  # runtime finds php8ts.dll
```

## Build

```powershell
cargo build --release --bin rapira
# -> target\release\rapira.exe
```

## Run

`rapira` runs a resident worker script. Each request runs your handler with the
superglobals populated; state created outside the handler (autoloader, container,
connections) survives across requests.

```php
<?php
// app\worker.php
require __DIR__ . '/vendor/autoload.php';

$app = new App(); // booted once, reused for every request

$handler = static function (): void {
    header('Content-Type: text/plain');
    http_response_code(200);
    echo $app->handle($_SERVER['REQUEST_URI']);
};

while (rapira_handle_request($handler)) {
    gc_collect_cycles();
}
```

```powershell
.\target\release\rapira.exe serve app\worker.php --threads 8
curl http://127.0.0.1:8000/
```

`rapira_finish_request(): bool` flushes the response early; the handler can keep working
after it. Bare `rapira` prints help. `Ctrl+C`/`Ctrl+Break` drains in-flight requests and
exits; a second one forces exit.

### Options

| Option | Default | Description |
|---|---|---|
| `--config <PATH>` | none | Load settings from a `rapira.toml`. |
| `--listen <ADDR>` | `127.0.0.1:8000` | `host:port` or `:port` (all interfaces). A bare port is rejected. |
| `--threads <N>` | CPU count | PHP worker threads (ZTS). |
| `SCRIPT` | required¹ | PHP entry script. Overrides `pool.entrypoint`. |

¹ Required unless the config file sets `pool.entrypoint`. Precedence: **CLI > config > defaults**.

### Configuration file

```toml
[http]
listen = "127.0.0.1:8000"
server_name = "localhost"    # optional; SERVER_NAME reported to PHP
server_port = 8000           # optional; defaults to the listen TCP port
max_body_size_mb = 8         # optional; larger bodies get a 413

[pool]
threads = 4
entrypoint = "app\\worker.php"  # relative -> resolved against this file's directory
```

Unknown keys are rejected.

## Logging

`env_logger`-based, via `RUST_LOG` (targets: `rapira`, `ext`, `php`). `.\dev.ps1` sets the
PHP/LLVM env and forwards to cargo:

```powershell
.\dev.ps1 -Devel C:\php\php-8.4-devel-vs17-x64 -Runtime C:\php\php-8.4-ts-x64 test --workspace
```

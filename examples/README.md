# Examples

The directory has one script for each run mode and two dispatcher variants. Use a release build of `rapira.exe` or build one with `dev.ps1`.

Classic mode executes the script once for each request:

```powershell
.\rapira.exe serve --mode classic examples\classic.php
```

Worker mode keeps the script resident and runs a handler closure for each request:

```powershell
.\rapira.exe serve --mode worker examples\worker.php
```

Dispatcher mode is the default. The synchronous example handles one request at a time on a blocking `receive()` call:

```powershell
.\rapira.exe serve examples\dispatcher-sync.php
```

The Fiber example runs each request in a Fiber. It completes the active exchange before it calls `receive()` for the next request:

```powershell
.\rapira.exe serve examples\dispatcher-async.php
```

Each interpreter thread boots its own resident script. `pool.processes` or `--processes` sets the number of interpreter threads.

Each interpreter thread handles one active exchange. Use more interpreter threads to handle concurrent requests.

All examples listen on `127.0.0.1:8000` by default. The classic and worker examples answer every path. The dispatcher examples provide these routes:

```powershell
curl.exe http://127.0.0.1:8000/
curl.exe -d ping http://127.0.0.1:8000/echo
curl.exe http://127.0.0.1:8000/boom
curl.exe http://127.0.0.1:8000/nope
curl.exe http://127.0.0.1:8000/stream
```

Start an example with the complete configuration file:

```powershell
.\rapira.exe serve --config examples\rapira.toml
```

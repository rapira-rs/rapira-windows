# Examples

The directory has one script for each run mode and two dispatcher variants. Use a release build of `rapira.exe` or build one with `dev.ps1`.

The shipped configuration file runs `dispatcher-sync.php`. To run another example, set `http.pool.entrypoint` and `http.pool.mode` in `examples\rapira.toml` or in a copy of it:

```powershell
.\rapira.exe serve examples\rapira.toml
```

| Entrypoint             | Mode         | Behavior                                                                                  |
| ---------------------- | ------------ | ----------------------------------------------------------------------------------------- |
| `classic.php`          | `classic`    | Executes the script once for each request.                                                |
| `worker.php`           | `worker`     | Keeps the script resident and runs a handler closure for each request.                    |
| `dispatcher-sync.php`  | `dispatcher` | Keeps the script resident and handles one request at a time on a blocking `receive()`.    |
| `dispatcher-async.php` | `dispatcher` | Keeps the script resident and runs each request in a Fiber with `tryReceive()` between resumes. |

Each interpreter thread boots its own resident script. `http.pool.processes` sets the number of interpreter threads.

Each interpreter thread handles one active exchange. Use more interpreter threads to handle concurrent requests.

All examples listen on `127.0.0.1:8000` by default. The classic and worker examples answer every path. The dispatcher examples provide these routes:

```powershell
curl.exe http://127.0.0.1:8000/
curl.exe -d ping http://127.0.0.1:8000/echo
curl.exe http://127.0.0.1:8000/boom
curl.exe http://127.0.0.1:8000/nope
curl.exe http://127.0.0.1:8000/stream
```

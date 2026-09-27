# Examples

Use an installed `rapira.exe`, or build one with `dev.ps1 -Task build`. Each file shows one HTTP mode. The dispatcher has synchronous and asynchronous examples.

The shipped file runs `dispatcher-sync.php`. To run another example, set `http.pool.entrypoint` and `http.pool.mode` in `examples/rapira.toml` or in a copy of it:

```powershell
.\rapira.exe serve examples\rapira.toml
```

| Entrypoint             | Mode         | What it shows                                                            |
| ---------------------- | ------------ | ------------------------------------------------------------------------ |
| `classic.php`          | `classic`    | one script execution per request                                         |
| `worker.php`           | `worker`     | resident script, a handler closure runs per request                      |
| `dispatcher-sync.php`  | `dispatcher` | resident script, one request at a time on a blocking `receive()`         |
| `dispatcher-async.php` | `dispatcher` | resident script, a fiber per request with `tryReceive()` between resumes |

All of them listen on 127.0.0.1:8000 by default. Classic and worker answer any path; the dispatcher examples route:

```powershell
curl.exe http://127.0.0.1:8000/                # hello
curl.exe -d 'ping' http://127.0.0.1:8000/echo   # request body
curl.exe http://127.0.0.1:8000/boom            # handler failure: 500
curl.exe http://127.0.0.1:8000/nope            # unknown route: 404
curl.exe http://127.0.0.1:8000/stream          # chunked streaming
```

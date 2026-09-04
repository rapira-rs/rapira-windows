mod cache;

use std::path::PathBuf;

use bytes::Bytes;
use extension_api::{BoxError, BoxFuture, HttpRequest, HttpResponse, Middleware, Next, empty_body};
use http::{Method, StatusCode};
use http_body_util::{BodyExt, Empty};
use tower_http::services::ServeDir;
use tower_http::services::fs::DefaultServeDirFallback;

use cache::CachingBackend;

/// Serves files from a directory and passes each cache miss to the next middleware. A permission error or an invalid file name is also a miss. Any other read failure returns 500 and stops middleware processing.
pub struct StaticFiles {
    dir: ServeDir<DefaultServeDirFallback, CachingBackend>,
    forbid: Vec<String>,
}

impl StaticFiles {
    /// A relative `root` resolves against the process working directory.
    /// `forbid` holds file-name suffixes with a leading dot.
    /// The constructor lowercases them, so an uppercase entry still matches in `eligible`.
    pub fn new(root: PathBuf, forbid: Vec<String>) -> Self {
        Self::with_cache(root, forbid, CachingBackend::default())
    }

    /// Takes the cache from the caller, so a test can inspect it.
    fn with_cache(root: PathBuf, mut forbid: Vec<String>, cache: CachingBackend) -> Self {
        for entry in &mut forbid {
            entry.make_ascii_lowercase();
        }
        Self {
            // PHP handles directory URLs as application routes. The middleware does not add an implicit index.html file.
            dir: ServeDir::with_backend(root, cache).append_index_html_on_directories(false),
            forbid,
        }
    }

    /// The check runs on the decoded path because ServeDir percent-decodes before it reads the filesystem. A match on the raw path would accept `%2Ephp`.
    fn eligible(&self, path: &str) -> bool {
        let Ok(decoded) = percent_encoding::percent_decode_str(path).decode_utf8() else {
            return false;
        };
        // Windows accepts backslashes as separators, colons as stream names, and 8.3 aliases.
        // Reject these forms before the dotfile and suffix filters see a different name.
        if decoded.contains('\\') || decoded.contains(':') {
            return false;
        }
        if decoded.split('/').any(|segment| {
            segment.starts_with('.')
                || segment
                    .as_bytes()
                    .windows(2)
                    .any(|pair| pair[0] == b'~' && pair[1].is_ascii_digit())
        }) {
            return false;
        }
        // The last non-empty segment is the served file. The component iterator ignores trailing separators, so `/index.php%2F` still identifies index.php here.
        // Windows also ignores trailing dots and spaces in a file name.
        let file = decoded
            .rsplit('/')
            .find(|s| !s.is_empty())
            .unwrap_or_default()
            .trim_end_matches(['.', ' '])
            .to_ascii_lowercase();
        !self.forbid.iter().any(|ext| file.ends_with(ext.as_str()))
    }
}

/// The error kinds that mean there is no file to serve. `try_call` reports a missing path and an unreadable path this way. The backend reports a directory with `IsADirectory`.
/// https://docs.rs/tower-http/0.7.1/tower_http/services/struct.ServeDir.html#method.try_call
///
/// A `HEAD` probe also reports a bad file name here. An overlong path component gives `InvalidFilename`. A NUL byte gives `InvalidInput`. A `GET` answers 404 for both names.
fn is_miss(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::NotFound
            | std::io::ErrorKind::PermissionDenied
            | std::io::ErrorKind::IsADirectory
            | std::io::ErrorKind::InvalidFilename
            | std::io::ErrorKind::InvalidInput
    )
}

impl Middleware for StaticFiles {
    fn handle<'a>(&'a self, req: HttpRequest, next: Next) -> BoxFuture<'a, HttpResponse> {
        Box::pin(async move {
            if req.method() != Method::GET && req.method() != Method::HEAD {
                return next.run(req).await;
            }
            if !self.eligible(req.uri().path()) {
                return next.run(req).await;
            }

            // The probe contains only the request metadata. The original request remains unchanged so the fallback handler receives the `Peer` and `Protocol` extensions.
            let mut probe = http::Request::new(Empty::<Bytes>::new());
            *probe.method_mut() = req.method().clone();
            *probe.uri_mut() = req.uri().clone();
            *probe.headers_mut() = req.headers().clone();

            let mut dir = self.dir.clone();
            match dir.try_call(probe).await {
                Ok(res) if res.status() != StatusCode::NOT_FOUND => {
                    res.map(|b| b.map_err(|e| -> BoxError { Box::new(e) }).boxed_unsync())
                }
                // A directory URL returns 404 without a filesystem error. The request then passes to PHP.
                Ok(_) => next.run(req).await,
                Err(e) if is_miss(&e) => next.run(req).await,
                // An `Err` outside the miss kinds is a read failure, so the request must not pass to PHP.
                // https://docs.rs/tower-http/0.7.1/tower_http/services/struct.ServeDir.html#method.try_call
                Err(e) => {
                    tracing::error!(target: "http", "static probe failed for {}: {e}", req.uri().path());
                    http::Response::builder()
                        .status(StatusCode::INTERNAL_SERVER_ERROR)
                        .body(empty_body())
                        .unwrap()
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use extension_api::{Addr, Handler, Peer, Protocol};
    use http_body_util::Full;
    use std::net::SocketAddr;
    use std::sync::Arc;
    use std::time::{Duration, SystemTime};

    fn peer() -> Peer {
        let addr: SocketAddr = "127.0.0.1:9".parse().unwrap();
        Peer {
            remote: Addr::Inet(addr),
            server: Addr::Inet(addr),
            https: false,
            received_at: 0.0,
        }
    }

    /// Reports whether fallback preserved the request extensions and body.
    struct Fallthrough;

    impl Handler for Fallthrough {
        fn call<'a>(&'a self, req: HttpRequest) -> BoxFuture<'a, HttpResponse> {
            Box::pin(async move {
                let kept = req.extensions().get::<Peer>().is_some()
                    && req.extensions().get::<Protocol>().is_some();
                let body = req.into_body().collect().await.unwrap().to_bytes();
                http::Response::builder()
                    .status(200)
                    .header("x-handler", "php")
                    .header("x-extensions", if kept { "kept" } else { "lost" })
                    .body(Full::new(body).map_err(|e| match e {}).boxed_unsync())
                    .unwrap()
            })
        }
    }

    fn request(method: &str, path: &str, body: &str) -> HttpRequest {
        let mut req = http::Request::builder()
            .method(method)
            .uri(path)
            .body(
                Full::new(Bytes::from(body.to_owned()))
                    .map_err(|e| match e {})
                    .boxed_unsync(),
            )
            .unwrap();
        req.extensions_mut().insert(Protocol::Http);
        req.extensions_mut().insert(peer());
        req
    }

    async fn run(st: StaticFiles, req: HttpRequest) -> HttpResponse {
        run_shared(&Arc::new(st), req).await
    }

    /// A cache test sends more than one request to the same instance.
    async fn run_shared(st: &Arc<StaticFiles>, req: HttpRequest) -> HttpResponse {
        let chain: Arc<[Arc<dyn Middleware>]> =
            Arc::from(vec![Arc::clone(st) as Arc<dyn Middleware>]);
        Next::new(chain, Arc::new(Fallthrough)).run(req).await
    }

    /// Use whole seconds so the tests do not depend on subsecond timestamp precision.
    fn write_at(path: &std::path::Path, contents: &str, mtime_secs: u64) {
        std::fs::write(path, contents).unwrap();
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(mtime_secs))
            .unwrap();
    }

    fn root() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("styles.css"), "body{}").unwrap();
        std::fs::write(dir.path().join("data.bin"), "abcdefghij").unwrap();
        std::fs::write(dir.path().join("index.html"), "<h1>hi</h1>").unwrap();
        std::fs::write(dir.path().join("index.php"), "<?php secret();").unwrap();
        std::fs::write(dir.path().join(".env"), "SECRET=1").unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        std::fs::write(dir.path().join(".git").join("config"), "[core]").unwrap();
        std::fs::write(dir.path().join("Upper.PHP"), "<?php upper();").unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub").join("index.php"), "<?php sub();").unwrap();
        std::fs::create_dir(dir.path().join("assets")).unwrap();
        std::fs::write(dir.path().join("assets").join("a.css"), "a{}").unwrap();
        dir
    }

    fn static_files(dir: &tempfile::TempDir) -> StaticFiles {
        StaticFiles::new(dir.path().to_path_buf(), vec![".php".to_owned()])
    }

    /// A cache test needs the instance and access to its store.
    fn cached(dir: &tempfile::TempDir) -> (Arc<StaticFiles>, CachingBackend) {
        let cache = CachingBackend::default();
        let st = StaticFiles::with_cache(
            dir.path().to_path_buf(),
            vec![".php".to_owned()],
            cache.clone(),
        );
        (Arc::new(st), cache)
    }

    fn header<'r>(res: &'r HttpResponse, name: &str) -> &'r str {
        res.headers()
            .get(name)
            .unwrap_or_else(|| panic!("{name} missing"))
            .to_str()
            .unwrap()
    }

    async fn body(res: HttpResponse) -> Bytes {
        res.into_body().collect().await.unwrap().to_bytes()
    }

    #[tokio::test(flavor = "current_thread")]
    async fn serves_a_file_with_type_length_and_validators() {
        let dir = root();
        let res = run(static_files(&dir), request("GET", "/styles.css", "")).await;
        assert_eq!(res.status(), 200);
        assert!(res.headers().get("x-handler").is_none());
        assert_eq!(header(&res, "content-type"), "text/css");
        assert_eq!(header(&res, "content-length"), "6");
        assert_eq!(header(&res, "accept-ranges"), "bytes");
        assert!(res.headers().contains_key("etag"));
        assert!(res.headers().contains_key("last-modified"));
        assert_eq!(body(res).await, "body{}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_miss_falls_through_with_the_request_intact() {
        let dir = root();
        let res = run(static_files(&dir), request("GET", "/nope.txt", "ping")).await;
        assert_eq!(header(&res, "x-handler"), "php");
        assert_eq!(header(&res, "x-extensions"), "kept");
        assert_eq!(body(res).await, "ping");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn non_get_head_methods_fall_through() {
        let dir = root();
        let res = run(static_files(&dir), request("POST", "/styles.css", "p")).await;
        assert_eq!(header(&res, "x-handler"), "php");
        assert_eq!(body(res).await, "p");
    }

    /// The encoded slash forms decode to a trailing separator that `ServeDir` ignores when it resolves the file.
    #[tokio::test(flavor = "current_thread")]
    async fn forbidden_extensions_fall_through_even_when_the_file_exists() {
        let dir = root();
        for path in [
            "/index.php",
            "/index%2Ephp",
            "/Upper.PHP",
            "/index.php%2F",
            "/index.php%2F%2F",
            "/sub/index.php%2F",
        ] {
            let res = run(static_files(&dir), request("GET", path, "")).await;
            assert_eq!(header(&res, "x-handler"), "php", "{path}");
        }
    }

    /// The constructor lowercases the entries, so an uppercase entry still blocks the PHP source.
    #[tokio::test(flavor = "current_thread")]
    async fn uppercase_forbid_needles_are_normalized() {
        let dir = root();
        let st = StaticFiles::new(dir.path().to_path_buf(), vec![".PHP".to_owned()]);
        let res = run(st, request("GET", "/index.php", "")).await;
        assert_eq!(header(&res, "x-handler"), "php");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn directory_paths_without_a_trailing_slash_fall_through() {
        let dir = root();
        for path in ["/assets", "/sub"] {
            let res = run(static_files(&dir), request("GET", path, "")).await;
            assert_eq!(header(&res, "x-handler"), "php", "{path}");
        }

        let res = run(static_files(&dir), request("GET", "/assets/a.css", "")).await;
        assert_eq!(res.status(), 200);
        assert_eq!(body(res).await, "a{}");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn directory_paths_with_a_trailing_slash_fall_through() {
        let dir = root();
        std::fs::write(dir.path().join("sub").join("index.html"), "<h1>s</h1>").unwrap();
        for path in ["/assets/", "/sub/"] {
            let res = run(static_files(&dir), request("GET", path, "")).await;
            assert_eq!(header(&res, "x-handler"), "php", "{path}");
        }
    }

    /// An empty forbid list allows the middleware to serve PHP source by its regular name.
    #[tokio::test(flavor = "current_thread")]
    async fn an_empty_forbid_list_serves_php_sources() {
        let dir = root();
        let st = StaticFiles::new(dir.path().to_path_buf(), Vec::new());
        let res = run(st, request("GET", "/index.php", "")).await;
        assert_eq!(res.status(), 200);
        assert!(res.headers().get("x-handler").is_none());
        assert_eq!(body(res).await, "<?php secret();");
    }

    /// An unsatisfiable byte range answers 416 (RFC 9110 section 15.5.17, https://www.rfc-editor.org/rfc/rfc9110#section-15.5.17).
    /// The response contains unsatisfied-range `"*/" complete-length` from RFC 9110 section 14.4: https://www.rfc-editor.org/rfc/rfc9110#section-14.4
    /// The file exists, so the middleware answers and PHP does not see the request.
    #[tokio::test(flavor = "current_thread")]
    async fn an_unsatisfiable_range_answers_416_without_reaching_php() {
        let dir = root();
        let mut req = request("GET", "/data.bin", "");
        req.headers_mut()
            .insert("range", "bytes=100-200".parse().unwrap());
        let res = run(static_files(&dir), req).await;
        assert_eq!(res.status(), 416);
        assert_eq!(header(&res, "content-range"), "bytes */10");
        assert!(res.headers().get("x-handler").is_none());
    }

    /// A symlink loop is a read failure outside the miss kinds. Creating the link requires developer mode or the symbolic-link privilege on Windows.
    #[tokio::test(flavor = "current_thread")]
    async fn a_real_read_failure_answers_500() {
        let dir = root();
        if let Err(err) =
            std::os::windows::fs::symlink_file("loop.css", dir.path().join("loop.css"))
        {
            if err.raw_os_error() == Some(1314) {
                return;
            }
            panic!("create symlink loop: {err}");
        }
        let res = run(static_files(&dir), request("GET", "/loop.css", "")).await;
        assert_eq!(res.status(), 500);
        assert!(res.headers().get("x-handler").is_none());
        assert_eq!(body(res).await, "");
    }

    /// When the filesystem rejects an overlong path component, the request passes to the next middleware.
    #[tokio::test(flavor = "current_thread")]
    async fn an_overlong_segment_falls_through() {
        let dir = root();
        let long = format!("/{}", "a".repeat(300));
        let res = run(static_files(&dir), request("GET", &long, "")).await;
        assert_eq!(header(&res, "x-handler"), "php");
    }

    /// A NUL byte reaches the file name case only for `HEAD`. A `GET` already returns 404.
    #[tokio::test(flavor = "current_thread")]
    async fn a_nul_byte_in_the_path_falls_through() {
        let dir = root();
        for method in ["GET", "HEAD"] {
            let res = run(static_files(&dir), request(method, "/a%00.css", "")).await;
            assert_eq!(header(&res, "x-handler"), "php", "{method}");
        }
    }

    /// The middleware cannot check an undecodable path against the dot and forbid rules, so the path is never eligible.
    #[test]
    fn an_undecodable_path_is_never_eligible() {
        let dir = root();
        assert!(!static_files(&dir).eligible("/%FF.css"));
    }

    #[test]
    fn encoded_backslash_is_never_eligible() {
        let st = StaticFiles::new(PathBuf::new(), Vec::new());
        assert!(!st.eligible("/%5C.env"));
    }

    #[test]
    fn trailing_dot_keeps_the_forbidden_suffix() {
        let st = StaticFiles::new(PathBuf::new(), vec![".php".to_owned()]);
        assert!(!st.eligible("/index.php."));
    }

    #[test]
    fn alternate_data_stream_is_never_eligible() {
        let st = StaticFiles::new(PathBuf::new(), Vec::new());
        assert!(!st.eligible("/index.php::$DATA"));
    }

    #[test]
    fn short_php_name_is_never_eligible_without_forbid() {
        let st = StaticFiles::new(PathBuf::new(), Vec::new());
        assert!(!st.eligible("/INDEX~1.PHP"));
    }

    #[test]
    fn short_dotfile_name_is_never_eligible() {
        let st = StaticFiles::new(PathBuf::new(), Vec::new());
        assert!(!st.eligible("/ENV~1"));
    }

    /// The same rule rejects parent directory segments because `..` starts with a dot.
    #[tokio::test(flavor = "current_thread")]
    async fn dotfile_segments_fall_through() {
        let dir = root();
        for path in [
            "/.env",
            "/.git/config",
            "/%2Eenv",
            "/../outside.txt",
            "/%2e%2e/outside.txt",
        ] {
            let res = run(static_files(&dir), request("GET", path, "")).await;
            assert_eq!(header(&res, "x-handler"), "php", "{path}");
        }
    }

    /// Byte positions are inclusive (RFC 9110 section 14.1.2, https://www.rfc-editor.org/rfc/rfc9110#section-14.1.2).
    /// Content-Range contains first-pos "-" last-pos "/" complete-length from RFC 9110 section 14.4: https://www.rfc-editor.org/rfc/rfc9110#section-14.4
    #[tokio::test(flavor = "current_thread")]
    async fn a_range_request_answers_the_named_bytes() {
        let dir = root();
        let mut req = request("GET", "/data.bin", "");
        req.headers_mut()
            .insert("range", "bytes=0-4".parse().unwrap());
        let res = run(static_files(&dir), req).await;
        assert_eq!(res.status(), 206);
        assert_eq!(header(&res, "content-range"), "bytes 0-4/10");
        assert_eq!(header(&res, "content-length"), "5");
        assert_eq!(body(res).await, "abcde");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn if_none_match_with_the_served_etag_answers_304() {
        let dir = root();
        let first = run(static_files(&dir), request("GET", "/data.bin", "")).await;
        let etag = header(&first, "etag").to_owned();

        let mut req = request("GET", "/data.bin", "");
        req.headers_mut()
            .insert("if-none-match", etag.parse().unwrap());
        let res = run(static_files(&dir), req).await;
        assert_eq!(res.status(), 304);
        assert_eq!(body(res).await, "");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn head_answers_the_full_length_without_a_body() {
        let dir = root();
        let res = run(static_files(&dir), request("HEAD", "/data.bin", "")).await;
        assert_eq!(res.status(), 200);
        assert_eq!(header(&res, "content-length"), "10");
        assert_eq!(body(res).await, "");
    }

    /// PHP handles directory URLs. The middleware serves only exact file paths and does not resolve index files.
    #[tokio::test(flavor = "current_thread")]
    async fn the_root_falls_through_even_with_an_index_present() {
        let dir = root();
        let res = run(static_files(&dir), request("GET", "/", "")).await;
        assert_eq!(header(&res, "x-handler"), "php");

        let res = run(static_files(&dir), request("GET", "/index.html", "")).await;
        assert_eq!(res.status(), 200);
        assert_eq!(body(res).await, "<h1>hi</h1>");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn query_strings_do_not_affect_resolution() {
        let dir = root();
        let res = run(static_files(&dir), request("GET", "/styles.css?v=2", "")).await;
        assert_eq!(res.status(), 200);
        assert_eq!(body(res).await, "body{}");
    }

    /// The test deletes the file after the first request. The second response must use the cached body.
    #[tokio::test(flavor = "current_thread")]
    async fn a_second_request_serves_from_memory() {
        let dir = root();
        let (st, _cache) = cached(&dir);
        let first = run_shared(&st, request("GET", "/styles.css", "")).await;
        let etag = header(&first, "etag").to_owned();
        let modified = header(&first, "last-modified").to_owned();
        std::fs::remove_file(dir.path().join("styles.css")).unwrap();

        let res = run_shared(&st, request("GET", "/styles.css", "")).await;
        assert_eq!(res.status(), 200);
        assert!(res.headers().get("x-handler").is_none());
        assert_eq!(header(&res, "etag"), etag);
        assert_eq!(header(&res, "last-modified"), modified);
        assert_eq!(header(&res, "content-type"), "text/css");
        assert_eq!(header(&res, "content-length"), "6");
        assert_eq!(body(res).await, "body{}");
    }

    /// The range starts past the first byte, so the answer needs a seek in the cached body.
    /// Content-Range contains first-pos "-" last-pos "/" complete-length from RFC 9110 section 14.4: https://www.rfc-editor.org/rfc/rfc9110#section-14.4
    #[tokio::test(flavor = "current_thread")]
    async fn a_cached_entry_still_serves_a_range() {
        let dir = root();
        let (st, _cache) = cached(&dir);
        run_shared(&st, request("GET", "/data.bin", "")).await;
        std::fs::remove_file(dir.path().join("data.bin")).unwrap();

        let mut req = request("GET", "/data.bin", "");
        req.headers_mut()
            .insert("range", "bytes=6-9".parse().unwrap());
        let res = run_shared(&st, req).await;
        assert_eq!(res.status(), 206);
        assert_eq!(header(&res, "content-range"), "bytes 6-9/10");
        assert_eq!(body(res).await, "ghij");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_cached_entry_still_answers_304() {
        let dir = root();
        let (st, _cache) = cached(&dir);
        let first = run_shared(&st, request("GET", "/data.bin", "")).await;
        let etag = header(&first, "etag").to_owned();
        std::fs::remove_file(dir.path().join("data.bin")).unwrap();

        let mut req = request("GET", "/data.bin", "");
        req.headers_mut()
            .insert("if-none-match", etag.parse().unwrap());
        let res = run_shared(&st, req).await;
        assert_eq!(res.status(), 304);
        assert_eq!(body(res).await, "");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn head_serves_cached_metadata() {
        let dir = root();
        let (st, _cache) = cached(&dir);
        run_shared(&st, request("GET", "/data.bin", "")).await;
        std::fs::remove_file(dir.path().join("data.bin")).unwrap();

        let res = run_shared(&st, request("HEAD", "/data.bin", "")).await;
        assert_eq!(res.status(), 200);
        assert_eq!(header(&res, "content-length"), "10");
        assert_eq!(body(res).await, "");
    }

    /// Both bodies have six bytes. Only the mtime shows the difference.
    #[tokio::test(flavor = "current_thread")]
    async fn a_stale_entry_reloads_a_same_length_rewrite() {
        let dir = root();
        let path = dir.path().join("rewrite.css");
        write_at(&path, "aaaaaa", 1_000_000);
        let (st, cache) = cached(&dir);

        let first = run_shared(&st, request("GET", "/rewrite.css", "")).await;
        let etag = header(&first, "etag").to_owned();
        assert_eq!(body(first).await, "aaaaaa");

        write_at(&path, "bbbbbb", 1_000_002);
        let inside = run_shared(&st, request("GET", "/rewrite.css", "")).await;
        assert_eq!(header(&inside, "etag"), etag);
        assert_eq!(body(inside).await, "aaaaaa");

        cache.advance_past_ttl();
        let after = run_shared(&st, request("GET", "/rewrite.css", "")).await;
        assert_ne!(header(&after, "etag"), etag);
        assert_eq!(body(after).await, "bbbbbb");
    }

    /// Both writes keep the mtime. Only the length shows the difference.
    #[tokio::test(flavor = "current_thread")]
    async fn a_stale_entry_reloads_a_same_mtime_resize() {
        let dir = root();
        let path = dir.path().join("resize.css");
        write_at(&path, "aaaaaa", 1_000_000);
        let (st, cache) = cached(&dir);
        let first = run_shared(&st, request("GET", "/resize.css", "")).await;
        assert_eq!(body(first).await, "aaaaaa");

        write_at(&path, "aaaaaaaaa", 1_000_000);
        cache.advance_past_ttl();
        let after = run_shared(&st, request("GET", "/resize.css", "")).await;
        assert_eq!(header(&after, "content-length"), "9");
        assert_eq!(body(after).await, "aaaaaaaaa");
    }

    /// A `HEAD` revalidation calls `Backend::metadata` but does not call `Backend::open`. The cached body must remain available, so a request after deletion still receives it from memory.
    #[tokio::test(flavor = "current_thread")]
    async fn revalidation_of_an_unchanged_file_keeps_the_body() {
        let dir = root();
        let (st, cache) = cached(&dir);
        run_shared(&st, request("GET", "/data.bin", "")).await;

        cache.advance_past_ttl();
        let head = run_shared(&st, request("HEAD", "/data.bin", "")).await;
        assert_eq!(head.status(), 200);
        assert_eq!(cache.entries(), 1, "revalidation must keep the body");
        assert_eq!(
            cache.reads(),
            1,
            "revalidation must not read the file again"
        );

        std::fs::remove_file(dir.path().join("data.bin")).unwrap();
        let res = run_shared(&st, request("GET", "/data.bin", "")).await;
        assert_eq!(res.status(), 200);
        assert_eq!(body(res).await, "abcdefghij");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_deleted_file_falls_through_after_the_ttl() {
        let dir = root();
        let (st, cache) = cached(&dir);
        assert_eq!(
            run_shared(&st, request("GET", "/styles.css", ""))
                .await
                .status(),
            200
        );
        std::fs::remove_file(dir.path().join("styles.css")).unwrap();
        assert_eq!(
            run_shared(&st, request("GET", "/styles.css", ""))
                .await
                .status(),
            200
        );

        cache.advance_past_ttl();
        let res = run_shared(&st, request("GET", "/styles.css", "")).await;
        assert_eq!(header(&res, "x-handler"), "php");
        assert_eq!(cache.entries(), 0);
        assert_eq!(cache.accounted(), 0);
    }

    /// The cache must store an empty body as an entry. Otherwise, the second request would access the filesystem.
    #[tokio::test(flavor = "current_thread")]
    async fn an_empty_file_is_cached() {
        let dir = root();
        std::fs::write(dir.path().join("empty.css"), "").unwrap();
        let (st, cache) = cached(&dir);
        assert_eq!(
            run_shared(&st, request("GET", "/empty.css", ""))
                .await
                .status(),
            200
        );
        assert_eq!(cache.entries(), 1);
        std::fs::remove_file(dir.path().join("empty.css")).unwrap();

        let res = run_shared(&st, request("GET", "/empty.css", "")).await;
        assert_eq!(res.status(), 200);
        assert_eq!(header(&res, "content-length"), "0");
        assert_eq!(body(res).await, "");
    }

    /// The cache has no entry for the file, so `HEAD` and `GET` both read the filesystem and return the same result under RFC 9110 section 9.3.2: https://www.rfc-editor.org/rfc/rfc9110#section-9.3.2
    #[tokio::test(flavor = "current_thread")]
    async fn a_file_over_the_cap_is_streamed_and_never_stored() {
        let dir = root();
        std::fs::write(dir.path().join("big.bin"), vec![b'x'; 262_145]).unwrap();
        let (st, cache) = cached(&dir);

        let res = run_shared(&st, request("GET", "/big.bin", "")).await;
        assert_eq!(res.status(), 200);
        assert_eq!(header(&res, "content-length"), "262145");
        assert_eq!(body(res).await.len(), 262_145);
        assert_eq!(cache.entries(), 0, "the cache must not store a large file");
        assert_eq!(cache.reads(), 0, "a large file must not reach the heap");

        std::fs::remove_file(dir.path().join("big.bin")).unwrap();
        let head = run_shared(&st, request("HEAD", "/big.bin", "")).await;
        let get = run_shared(&st, request("GET", "/big.bin", "")).await;
        assert_eq!(header(&head, "x-handler"), "php");
        assert_eq!(header(&get, "x-handler"), "php");
    }

    /// A limit of 16 MiB stores 63 entries of 256 KiB because a 64th entry would require a negative path length. `ServeDir` streams later files from disk, and the cache does not read them into memory. All entries then expire, so the last file fits.
    #[tokio::test(flavor = "current_thread")]
    async fn a_full_cache_stops_storing_and_reclaims_after_the_ttl() {
        let dir = root();
        for i in 0..70 {
            std::fs::write(dir.path().join(format!("f{i:02}.bin")), vec![b'y'; 262_144]).unwrap();
        }
        let (st, cache) = cached(&dir);
        for i in 0..70 {
            let res = run_shared(&st, request("GET", &format!("/f{i:02}.bin"), "")).await;
            assert_eq!(res.status(), 200, "f{i:02}.bin");
            assert_eq!(body(res).await.len(), 262_144, "f{i:02}.bin");
        }

        assert_eq!(cache.entries(), 63);
        assert_eq!(cache.reads(), 63, "a refused file must not reach the heap");
        assert!(cache.accounted() <= 16 * 1024 * 1024);
        assert_eq!(cache.accounted(), cache.recomputed());

        cache.advance_past_ttl();
        run_shared(&st, request("GET", "/f69.bin", "")).await;
        assert_eq!(cache.entries(), 1, "a stale entry must release its room");
        assert_eq!(cache.accounted(), cache.recomputed());
    }

    /// A replacement or removal can cause a mismatch in the running total. The test performs both operations and compares the total with the sum of the entries.
    #[tokio::test(flavor = "current_thread")]
    async fn the_byte_total_tracks_the_map() {
        let dir = root();
        let styles = dir.path().join("styles.css");
        write_at(&styles, "body{}", 1_000_000);
        let (st, cache) = cached(&dir);
        run_shared(&st, request("GET", "/styles.css", "")).await;
        run_shared(&st, request("GET", "/data.bin", "")).await;
        assert_eq!(cache.accounted(), cache.recomputed());

        write_at(&styles, "body{color:red}", 1_000_002);
        std::fs::remove_file(dir.path().join("data.bin")).unwrap();
        cache.advance_past_ttl();

        let res = run_shared(&st, request("GET", "/styles.css", "")).await;
        assert_eq!(header(&res, "content-length"), "15");
        assert_eq!(body(res).await, "body{color:red}");
        let res = run_shared(&st, request("GET", "/data.bin", "")).await;
        assert_eq!(header(&res, "x-handler"), "php");

        assert_eq!(cache.entries(), 1);
        assert_eq!(cache.accounted(), cache.recomputed());
    }

    /// A directory URL is a PHP route. The cache has no entry for it, so `HEAD` and `GET` read the same filesystem state.
    #[tokio::test(flavor = "current_thread")]
    async fn a_directory_is_not_cached() {
        let dir = root();
        let (st, cache) = cached(&dir);
        for _ in 0..2 {
            let res = run_shared(&st, request("GET", "/sub", "")).await;
            assert_eq!(header(&res, "x-handler"), "php");
        }
        assert_eq!(cache.entries(), 0);
    }

    /// Eight requests use one uncached path. One task reads the file into the cache, and the other tasks stream it from disk. All responses match, and the cache reads the file once.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_fills_agree() {
        let dir = root();
        let (st, cache) = cached(&dir);
        let gate = Arc::new(tokio::sync::Barrier::new(8));
        let mut tasks = Vec::new();
        for _ in 0..8 {
            let st = Arc::clone(&st);
            let gate = Arc::clone(&gate);
            tasks.push(tokio::spawn(async move {
                gate.wait().await;
                let res = run_shared(&st, request("GET", "/data.bin", "")).await;
                let etag = header(&res, "etag").to_owned();
                (etag, body(res).await)
            }));
        }

        let mut answers = Vec::new();
        for task in tasks {
            answers.push(task.await.unwrap());
        }
        for (etag, bytes) in &answers {
            assert_eq!(etag, &answers[0].0);
            assert_eq!(bytes, "abcdefghij");
        }
        assert_eq!(cache.entries(), 1);
        assert_eq!(cache.reads(), 1, "one task reads the file into memory");
        assert_eq!(cache.accounted(), cache.recomputed());
    }
}

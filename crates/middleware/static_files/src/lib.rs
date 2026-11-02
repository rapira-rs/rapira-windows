mod cache;
mod config;

pub use config::{Section, Settings, resolve};

use std::path::PathBuf;

use bytes::Bytes;
use http::{Method, StatusCode, header};
use http_body_util::combinators::UnsyncBoxBody;
use http_body_util::{BodyExt, Empty};
use tower_http::services::ServeDir;
use tower_http::services::fs::DefaultServeDirFallback;

use cache::CachingBackend;

type BoxError = Box<dyn std::error::Error + Send + Sync>;
/// The body of a static-file response.
type Body = UnsyncBoxBody<Bytes, BoxError>;

/// Serves files from a directory. The HTTP plugin sends each miss to PHP.
/// A permission error or a bad file name is also a miss. Other read errors return 500.
pub struct StaticFiles {
    dir: ServeDir<DefaultServeDirFallback, CachingBackend>,
    forbid: Vec<String>,
}

impl StaticFiles {
    /// A relative `root` resolves against the process working directory.
    /// `forbid` holds file-name suffixes with a leading dot.
    /// The constructor lowercases them, so an uppercase entry still matches in `eligible`.
    pub fn new(root: PathBuf, mut forbid: Vec<String>) -> Self {
        for entry in &mut forbid {
            entry.make_ascii_lowercase();
        }
        Self {
            // The URL space belongs to PHP: a directory URL is the app's route, not an
            // implicit index.html.
            dir: ServeDir::with_backend(root, CachingBackend::default())
                .append_index_html_on_directories(false)
                .ignore_multi_range_requests(true),
            forbid,
        }
    }

    /// The check runs on the decoded path because ServeDir percent-decodes before it reads
    /// the filesystem. A match on the raw path would accept `%2Ephp`.
    fn eligible(&self, path: &str) -> bool {
        let Ok(decoded) = percent_encoding::percent_decode_str(path).decode_utf8() else {
            return false;
        };
        // Reject Windows aliases before suffix filtering: ADS, alternate separators and 8.3 names.
        if decoded.contains(['\\', ':']) {
            return false;
        }
        if decoded.split('/').any(|segment| {
            segment.starts_with('.')
                || segment
                    .as_bytes()
                    .windows(2)
                    .any(|p| p[0] == b'~' && p[1].is_ascii_digit())
        }) {
            return false;
        }
        // The last non-empty segment is the served file; the component walk drops trailing
        // separators, so `/index.php%2F` still names index.php here.
        let file = decoded
            .rsplit('/')
            .find(|s| !s.is_empty())
            .unwrap_or_default()
            .trim_end_matches(['.', ' '])
            .to_ascii_lowercase();
        !self.forbid.iter().any(|ext| file.ends_with(ext.as_str()))
    }
}

/// The error kinds that mean there is no file to serve. `try_call` reports a missing path and
/// an unreadable path this way. The backend reports a directory with `IsADirectory`.
/// https://docs.rs/tower-http/0.7.1/tower_http/services/struct.ServeDir.html#method.try_call
///
/// A bad file name also reaches this check. A segment over `NAME_MAX` gives `InvalidFilename`.
/// A NUL byte gives `InvalidInput` on `HEAD`. A `GET` answers 404 for a NUL byte.
fn is_miss(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::NotFound
            | std::io::ErrorKind::PermissionDenied
            | std::io::ErrorKind::IsADirectory
            | std::io::ErrorKind::InvalidFilename
            | std::io::ErrorKind::InvalidInput
            | std::io::ErrorKind::NotADirectory
    )
}

impl StaticFiles {
    /// Returns None for a miss. The caller retains the request body for PHP.
    pub async fn serve(&self, req: &http::request::Parts) -> Option<http::Response<Body>> {
        if req.method != Method::GET && req.method != Method::HEAD {
            return None;
        }
        if !self.eligible(req.uri.path()) {
            return None;
        }

        let mut dir = self.dir.clone();
        let mut served = dir.try_call(probe(req)).await;
        // A different If-Range validator requires the whole file: https://www.rfc-editor.org/rfc/rfc9110#section-13.1.5
        if let Ok(res) = &served
            && res.headers().contains_key(header::CONTENT_RANGE)
            && let Some(validator) = req.headers.get(header::IF_RANGE)
            && res.headers().get(header::ETAG) != Some(validator)
        {
            let mut whole = probe(req);
            whole.headers_mut().remove(header::RANGE);
            served = dir.try_call(whole).await;
        }
        match served {
            Ok(res) if res.status() != StatusCode::NOT_FOUND => {
                Some(res.map(|b| b.map_err(|e| -> BoxError { Box::new(e) }).boxed_unsync()))
            }
            // A directory URL answers 404 without a filesystem error. It then reaches PHP.
            Ok(_) => None,
            Err(e) if is_miss(&e) => None,
            // An Err outside the miss kinds is a read failure and must not reach PHP.
            // https://docs.rs/tower-http/0.7.1/tower_http/services/struct.ServeDir.html#method.try_call
            Err(e) => {
                tracing::error!(target: "http", "static probe failed for {}: {e}", req.uri.path());
                Some(
                    http::Response::builder()
                        .status(StatusCode::INTERNAL_SERVER_ERROR)
                        .body(Empty::<Bytes>::new().map_err(BoxError::from).boxed_unsync())
                        .unwrap(),
                )
            }
        }
    }
}

fn probe(req: &http::request::Parts) -> http::Request<Empty<Bytes>> {
    let mut probe = http::Request::new(Empty::<Bytes>::new());
    *probe.method_mut() = req.method.clone();
    *probe.uri_mut() = req.uri.clone();
    *probe.headers_mut() = req.headers.clone();
    if req.method == Method::HEAD {
        probe.headers_mut().remove(header::RANGE);
    }
    probe
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The middleware cannot check an undecodable path against the dot and forbid rules, so the path is never eligible.
    #[test]
    fn an_undecodable_path_is_never_eligible() {
        let st = StaticFiles::new(PathBuf::from("/"), vec![".php".into()]);
        assert!(!st.eligible("/%FF.css"));
    }

    #[test]
    fn windows_alias_guards_match_the_decoded_path() {
        struct Case {
            name: &'static str,
            path: &'static str,
            allowed: bool,
        }
        let cases = [
            Case {
                name: "backslash separator",
                path: "/assets\\a.css",
                allowed: false,
            },
            Case {
                name: "encoded backslash separator",
                path: "/assets%5Ca.css",
                allowed: false,
            },
            Case {
                name: "alternate data stream",
                path: "/index.php:source.css",
                allowed: false,
            },
            Case {
                name: "encoded alternate data stream separator",
                path: "/index.php%3Asource.css",
                allowed: false,
            },
            Case {
                name: "short file name",
                path: "/secret~1.css",
                allowed: false,
            },
            Case {
                name: "encoded short file name",
                path: "/secret%7E1.css",
                allowed: false,
            },
            Case {
                name: "short directory name",
                path: "/assets~1/a.css",
                allowed: false,
            },
            Case {
                name: "forbidden suffix with a final dot",
                path: "/index.php.",
                allowed: false,
            },
            Case {
                name: "forbidden suffix with an encoded final dot",
                path: "/index.php%2E",
                allowed: false,
            },
            Case {
                name: "forbidden suffix with an encoded final space",
                path: "/index.php%20",
                allowed: false,
            },
            Case {
                name: "forbidden suffix with final dot and space",
                path: "/index.php.%20",
                allowed: false,
            },
            Case {
                name: "forward slash separator",
                path: "/assets/a.css",
                allowed: true,
            },
            Case {
                name: "hyphen after a forbidden extension",
                path: "/index.php-source.css",
                allowed: true,
            },
            Case {
                name: "tilde followed by a letter in a file name",
                path: "/secret~x.css",
                allowed: true,
            },
            Case {
                name: "tilde followed by a letter in a directory name",
                path: "/assets~x/a.css",
                allowed: true,
            },
            Case {
                name: "allowed extension after a forbidden extension",
                path: "/index.php.css",
                allowed: true,
            },
            Case {
                name: "allowed suffix with a final dot",
                path: "/app.css.",
                allowed: true,
            },
            Case {
                name: "allowed suffix with an encoded final space",
                path: "/app.css%20",
                allowed: true,
            },
        ];
        let st = StaticFiles::new(PathBuf::from("/"), vec![".php".into()]);
        for case in cases {
            assert_eq!(st.eligible(case.path), case.allowed, "{}", case.name);
        }
    }
}

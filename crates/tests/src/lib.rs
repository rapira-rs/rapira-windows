use http::{HeaderMap, HeaderName, HeaderValue};
use rapira_sapi::Frame;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

pub mod grpc;
pub mod metrics;
pub mod server_log;
pub mod wire;

/// Absolute path to a PHP fixture shipped with this crate (robust to the test's cwd).
pub fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(name)
}

pub type Fields = &'static [(&'static str, &'static str)];

/// A header map from static (name, value) lines.
pub fn fields(lines: Fields) -> HeaderMap {
    lines
        .iter()
        .map(|&(k, v)| (HeaderName::from_static(k), HeaderValue::from_static(v)))
        .collect()
}

/// Panics when RAPIRA_REQUIRE_EXTS names an extension this fixture covers: a skip where CI installs the extension is a broken install.
pub fn assert_skip_allowed(fixture: &str) {
    let Ok(required) = std::env::var("RAPIRA_REQUIRE_EXTS") else {
        return;
    };
    for ext in required.split(',').map(str::trim).filter(|e| !e.is_empty()) {
        assert!(
            !fixture.contains(ext),
            "{fixture} skipped, but RAPIRA_REQUIRE_EXTS demands {ext}"
        );
    }
}

/// A GET request for `uri` with `Host: localhost` and a raw body.
pub fn req(uri: &str) -> http::Request<Vec<u8>> {
    http::Request::builder()
        .uri(uri)
        .header(http::header::HOST, "localhost")
        .body(Vec::new())
        .expect("a valid test request")
}

/// Calls `check` every 50 ms until it returns Some or the timeout expires.
pub fn poll<T>(within: Duration, mut check: impl FnMut() -> Option<T>) -> Option<T> {
    let end = Instant::now() + within;
    loop {
        if let Some(v) = check() {
            return Some(v);
        }
        if Instant::now() >= end {
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The FileDescriptorSet of `fixtures/grpc/echo.proto`.
pub fn echo_descriptor_set() -> PathBuf {
    fixture("grpc/echo.binpb")
}

/// A response stream collected to its `End` (or to the producer dying).
#[derive(Debug, Default)]
pub struct Resp {
    pub head: Option<rapira_sapi::ResponseHead>,
    pub content_length: Option<u64>,
    pub body: Vec<u8>,
    pub truncated: bool,
    /// An `End` frame arrived; false = the producer died first.
    pub ended: bool,
}

impl Resp {
    /// 0 = no head (producer died, or the response never recorded one).
    pub fn status(&self) -> u16 {
        self.head.as_ref().map_or(0, |h| h.status)
    }

    pub fn header(&self, name: &str) -> Option<String> {
        let value = self.head.as_ref()?.headers.get(name)?;
        Some(String::from_utf8_lossy(value.as_bytes()).into_owned())
    }

    pub fn body_string(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    /// Fold one frame in; true when the stream is over.
    fn fold(&mut self, frame: Frame) -> bool {
        match frame {
            Frame::Head {
                head,
                content_length,
                ..
            } => {
                self.head = Some(head);
                self.content_length = content_length;
            }
            Frame::Chunk(b) => self.body.extend_from_slice(&b),
            Frame::End { truncated } => {
                self.truncated = truncated;
                self.ended = true;
                return true;
            }
            Frame::File { .. } => {
                unreachable!("the wire client sends only Head, Chunk and End")
            }
        }
        false
    }
}

/// Collect a response until `deadline`. Return None if the response does not finish in time.
pub fn drain_resp_deadline(
    rx: &mut mpsc::Receiver<Frame>,
    deadline: std::time::Instant,
) -> Option<Resp> {
    let mut resp = Resp::default();
    loop {
        if Instant::now() >= deadline {
            return None;
        }
        match rx.try_recv() {
            Ok(frame) => {
                if resp.fold(frame) {
                    return Some(resp);
                }
            }
            Err(mpsc::error::TryRecvError::Disconnected) => return Some(resp),
            Err(mpsc::error::TryRecvError::Empty) => {
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
}

pub fn drain_resp(mut rx: mpsc::Receiver<Frame>) -> Resp {
    drain_resp_deadline(&mut rx, Instant::now() + Duration::from_secs(60))
        .expect("response did not finish within 60 seconds")
}

/// Drain to `(status, body)`; status is 0 when no head was produced.
pub fn drain(rx: mpsc::Receiver<Frame>) -> (u16, String) {
    let r = drain_resp(rx);
    (r.status(), r.body_string())
}

pub async fn drain_resp_async(mut rx: mpsc::Receiver<Frame>) -> Resp {
    tokio::time::timeout(Duration::from_secs(60), async {
        let mut resp = Resp::default();
        while let Some(frame) = rx.recv().await {
            if resp.fold(frame) {
                break;
            }
        }
        resp
    })
    .await
    .expect("response did not finish within 60 seconds")
}

pub async fn drain_async(rx: mpsc::Receiver<Frame>) -> (u16, String) {
    let r = drain_resp_async(rx).await;
    (r.status(), r.body_string())
}

#[cfg(test)]
mod completion_tests {
    use super::*;

    #[test]
    fn a_response_deadline_applies_before_and_after_the_head() {
        struct Case {
            name: &'static str,
            head: bool,
        }
        let cases = [
            Case {
                name: "no response head",
                head: false,
            },
            Case {
                name: "head without completion",
                head: true,
            },
        ];
        for case in cases {
            let (sender, mut reply) = mpsc::channel(1);
            if case.head {
                sender
                    .try_send(Frame::Head {
                        head: rapira_sapi::ResponseHead {
                            status: 200,
                            headers: HeaderMap::new(),
                        },
                        content_length: Some(1),
                        bodiless: false,
                    })
                    .unwrap_or_else(|_| panic!("{}: send head", case.name));
            }
            let (completed, result) = std::sync::mpsc::channel();
            let reader = std::thread::spawn(move || {
                let deadline = Instant::now() + Duration::from_millis(20);
                completed
                    .send(drain_resp_deadline(&mut reply, deadline))
                    .unwrap();
            });
            let response = result.recv_timeout(Duration::from_secs(2));
            drop(sender);
            reader.join().expect("join response reader");
            assert!(matches!(response, Ok(None)), "{}", case.name);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn async_response_reads_fail_when_the_response_does_not_finish() {
        struct Case {
            name: &'static str,
            head: bool,
        }
        let cases = [
            Case {
                name: "no response head",
                head: false,
            },
            Case {
                name: "head without completion",
                head: true,
            },
        ];
        for case in cases {
            let (sender, reply) = mpsc::channel(1);
            if case.head {
                sender
                    .try_send(Frame::Head {
                        head: rapira_sapi::ResponseHead {
                            status: 200,
                            headers: HeaderMap::new(),
                        },
                        content_length: Some(1),
                        bodiless: false,
                    })
                    .unwrap_or_else(|_| panic!("{}: send head", case.name));
            }
            let response = tokio::time::timeout(
                Duration::from_secs(61),
                tokio::spawn(drain_resp_async(reply)),
            )
            .await;
            drop(sender);
            assert!(
                matches!(response, Ok(Err(error)) if error.is_panic()),
                "{}",
                case.name
            );
        }
    }
}

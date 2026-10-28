use crate::harness::{self, http_get};
use std::time::Duration;

const REQ: Duration = Duration::from_secs(10);

#[test]
fn callback_outcome_controls_response_completion() -> anyhow::Result<()> {
    struct Case {
        name: &'static str,
        query: &'static str,
        status: u16,
        body: &'static str,
        truncated: bool,
    }
    let cases = [
        Case {
            name: "exception handler",
            query: "/?exit=handler",
            status: 404,
            body: "Not Found\n",
            truncated: false,
        },
        Case {
            name: "fatal shutdown callback",
            query: "/?exit=shutdown-fatal",
            status: 200,
            body: "partial page\n",
            truncated: true,
        },
        Case {
            name: "shutdown callback",
            query: "/?exit=shutdown",
            status: 200,
            body: "ok\nbye\n",
            truncated: false,
        },
    ];
    let srv = harness::Spawn::http(
        rapira_sapi::Mode::Classic,
        harness::fixture_path("classic/exit-in-callback.php"),
    )
    .spawn();
    for case in &cases {
        let response = tests::drain_resp(tests::wire::submit(srv.addr, tests::req(case.query))?);
        assert_eq!(response.status(), case.status, "{}", case.name);
        assert_eq!(response.body_string(), case.body, "{}", case.name);
        assert!(response.ended, "{}", case.name);
        assert_eq!(response.truncated, case.truncated, "{}", case.name);
        assert_eq!(
            response.content_length,
            (!case.truncated).then_some(case.body.len() as u64),
            "{}",
            case.name
        );
    }
    harness::slot_line(&srv, "handled 3 errors 1 ");
    Ok(())
}

/// A worker enters the entrypoint directory once and stays there, so a userland chdir outlives the request that made it.
#[test]
fn classic_mode_keeps_the_working_directory_across_requests() {
    let srv = harness::spawn_with_config("classic/cwd.php", 1, "mode = \"classic\"\n");
    // getcwd() reports the resolved path, and the scratch dir can sit under a symlinked temp dir.
    let entrypoint_dir = std::fs::canonicalize(&srv.dir).expect("canonicalize the scratch dir");

    let (code, body) = http_get(srv.addr, "/", REQ).expect("request");
    assert_eq!(code, 200, "{}", harness::diagnostics(&srv));
    assert_eq!(
        std::fs::canonicalize(String::from_utf8_lossy(&body).as_ref()).unwrap(),
        entrypoint_dir,
        "the first request must run in the entrypoint directory\n{}",
        harness::diagnostics(&srv)
    );

    let (code, body) = http_get(srv.addr, "/chdir", REQ).expect("request");
    assert_eq!(code, 200, "{}", harness::diagnostics(&srv));
    let moved = String::from_utf8_lossy(&body).into_owned();
    assert_ne!(
        std::fs::canonicalize(&moved).unwrap(),
        entrypoint_dir,
        "the fixture must chdir out of the entrypoint directory\n{}",
        harness::diagnostics(&srv)
    );

    let (code, body) = http_get(srv.addr, "/", REQ).expect("request");
    assert_eq!(code, 200, "{}", harness::diagnostics(&srv));
    assert_eq!(
        String::from_utf8_lossy(&body),
        moved,
        "the chdir of the previous request must still hold\n{}",
        harness::diagnostics(&srv)
    );
}

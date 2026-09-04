use crate::harness::{self, http_get};
use std::time::Duration;

const REQ: Duration = Duration::from_secs(10);

/// Windows adds registry and Windows directory search locations. The SAPI excludes the working directory through php_ini_ignore_cwd.
#[test]
fn php_ini_in_the_working_directory_is_ignored() {
    let srv = harness::spawn_in_cwd("ini/precision.php", 1, "precision = 5\n");
    let (code, body) = http_get(srv.addr, "/", REQ).expect("request");
    assert_eq!(code, 200, "{}", harness::diagnostics(&srv));
    let body = String::from_utf8_lossy(&body);
    assert!(
        body.contains("precision=14"),
        "a php.ini in the cwd must not apply (got {body:?})\n{}",
        harness::diagnostics(&srv)
    );
}

/// Confirms that the fixture reads php.ini. This makes a successful working directory exclusion test valid.
#[test]
fn the_same_file_applies_through_phprc() {
    let srv = harness::spawn_with_phprc_and_config("ini/precision.php", 1, "precision = 5\n", "");
    let (code, body) = http_get(srv.addr, "/", REQ).expect("request");
    assert_eq!(code, 200, "{}", harness::diagnostics(&srv));
    let body = String::from_utf8_lossy(&body);
    assert!(
        body.contains("precision=5"),
        "PHPRC must apply (got {body:?})\n{}",
        harness::diagnostics(&srv)
    );
}

/// An explicit PHPRC file is checked before the normal Windows search locations.
#[test]
fn a_phprc_file_with_spaces_wins_over_the_working_directory_ini() {
    let srv = harness::spawn_with_phprc_file(
        "ini/precision.php",
        1,
        "precision = 5\n",
        "selected config/selected.ini",
        "precision = 7\n",
    );
    let (code, body) = http_get(srv.addr, "/", REQ).expect("request");
    assert_eq!(code, 200, "{}", harness::diagnostics(&srv));
    assert_eq!(
        body.as_slice(),
        b"precision=7",
        "the PHPRC file must win over cwd and default search paths\n{}",
        harness::diagnostics(&srv)
    );
}

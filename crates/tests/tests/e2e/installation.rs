use std::path::PathBuf;
use std::time::Duration;

use rapira_sapi::Mode;
use serde_json::Value;
use tests::fixture;

use crate::harness::{Spawn, diagnostics, http_get};

#[test]
#[ignore = "requires an installed release bundle"]
fn installed_bundle_uses_its_php_and_extensions() {
    let binary = PathBuf::from(std::env::var_os("RAPIRA_BIN").expect("installed RAPIRA_BIN"));
    let version = std::fs::read_to_string(binary.with_file_name("PHP_VERSION.txt"))
        .expect("installed PHP version");
    let srv = Spawn::http(Mode::Worker, fixture("installation/runtime-worker.php"))
        .env("PHPRC", "")
        .env("PHP_INI_SCAN_DIR", "")
        .spawn();
    let (status, body) = http_get(srv.addr, "/", Duration::from_secs(10)).expect("request");
    assert_eq!(status, 200, "{}", diagnostics(&srv));
    let runtime: Value = serde_json::from_slice(&body).expect("PHP runtime response");
    assert_eq!(runtime["version"], version.trim());
    assert_eq!(runtime["zts"], 1);
    let extensions = runtime["extensions"].as_array().expect("loaded extensions");
    for extension in include_str!("../../../../ci/windows-extensions.txt")
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
    {
        assert!(
            extensions.iter().any(|loaded| loaded == extension),
            "missing extension: {extension}\n{}",
            diagnostics(&srv)
        );
    }
}

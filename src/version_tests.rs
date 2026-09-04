use clap::CommandFactory;

use crate::Cli;

const PACKAGE_MANIFESTS: &[(&str, &str)] = &[
    ("rapira_windows", include_str!("../Cargo.toml")),
    ("extension_api", include_str!("../crates/api/Cargo.toml")),
    ("rapira_config", include_str!("../crates/config/Cargo.toml")),
    (
        "rapira_static_files",
        include_str!("../crates/middleware/static_files/Cargo.toml"),
    ),
    ("php_sys", include_str!("../crates/php_sys/Cargo.toml")),
    (
        "rapira_http",
        include_str!("../crates/plugins/http/Cargo.toml"),
    ),
    (
        "rapira_runtime",
        include_str!("../crates/runtime/Cargo.toml"),
    ),
    (
        "rapira_scoreboard",
        include_str!("../crates/scoreboard/Cargo.toml"),
    ),
    ("tests", include_str!("../crates/tests/Cargo.toml")),
];

fn package_version(manifest: &str) -> Option<&str> {
    let mut in_package = false;
    for line in manifest.lines().map(str::trim) {
        if line.starts_with('[') {
            in_package = line == "[package]";
            continue;
        }
        if in_package
            && let Some(version) = line
                .strip_prefix("version = \"")
                .and_then(|value| value.strip_suffix('"'))
        {
            return Some(version);
        }
    }
    None
}

#[test]
fn workspace_packages_use_the_product_version() {
    let product_version = env!("CARGO_PKG_VERSION");
    for (name, manifest) in PACKAGE_MANIFESTS {
        assert_eq!(
            package_version(manifest),
            Some(product_version),
            "{name} must use the product version"
        );
    }
}

#[test]
fn cli_uses_the_product_version() {
    assert_eq!(
        Cli::command().get_version(),
        Some(env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn php_api_uses_the_product_version() {
    let mut len = 0;
    // SAFETY: `len` is writable, and the function returns a static string with this byte length.
    let ptr = unsafe { php_sys::dispatcher::rapira_rs_version(&raw mut len) };
    assert!(!ptr.is_null());
    // SAFETY: The function returned a non-null pointer to `len` initialized bytes.
    let version = unsafe { std::slice::from_raw_parts(ptr.cast::<u8>(), len) };
    assert_eq!(version, env!("CARGO_PKG_VERSION").as_bytes());
}

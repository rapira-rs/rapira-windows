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

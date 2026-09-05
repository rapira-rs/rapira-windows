use tests::{assert_skip_allowed, run_worker};

fn check_extension(name: &str, token: &str) -> anyhow::Result<()> {
    let out = run_worker(name, &["/", "/?boom=1", "/"])?;
    if out[0].1 == "skip" {
        assert_skip_allowed(name);
        return Ok(());
    }
    assert_eq!(
        out[1].0, 500,
        "{name} uncaught throw must be a 500 (got: {:?})",
        out[1]
    );
    for index in [0, 2] {
        assert_eq!(
            out[index].0, 200,
            "{name} request {index}: {:?}",
            out[index]
        );
        assert!(
            out[index].1.contains(token),
            "{name} request {index} must echo {token:?}: {:?}",
            out[index].1
        );
    }
    Ok(())
}

#[test]
fn zlib_success_and_exception() -> anyhow::Result<()> {
    check_extension("php_ext/zlib-worker.php", "zlib:rapira zlib")
}

#[test]
fn curl_success_and_exception() -> anyhow::Result<()> {
    check_extension("php_ext/curl-worker.php", "curl:")
}

#[test]
fn ctype_success_and_exception() -> anyhow::Result<()> {
    check_extension("php_ext/ctype-worker.php", "ctype:1")
}

#[test]
fn mbstring_success_and_exception() -> anyhow::Result<()> {
    check_extension("php_ext/mbstring-worker.php", "mb:HÉLLO")
}

#[test]
fn iconv_success_and_exception() -> anyhow::Result<()> {
    check_extension("php_ext/iconv-worker.php", "iconv:iconv ok")
}

#[test]
fn openssl_success_and_exception() -> anyhow::Result<()> {
    check_extension("php_ext/openssl-worker.php", "openssl:64")
}

#[test]
fn fileinfo_success_and_exception() -> anyhow::Result<()> {
    check_extension("php_ext/fileinfo-worker.php", "finfo:text/plain")
}

#[test]
fn tokenizer_success_and_exception() -> anyhow::Result<()> {
    check_extension("php_ext/tokenizer-worker.php", "tok:")
}

#[test]
fn phar_success_and_exception() -> anyhow::Result<()> {
    check_extension("php_ext/phar-worker.php", "phar:")
}

#[test]
fn dom_success_and_exception() -> anyhow::Result<()> {
    check_extension("php_ext/dom-worker.php", "dom:ok")
}

#[test]
fn simplexml_success_and_exception() -> anyhow::Result<()> {
    check_extension("php_ext/simplexml-worker.php", "sxml:ok")
}

#[test]
fn xml_success_and_exception() -> anyhow::Result<()> {
    check_extension("php_ext/xml-worker.php", "xml:1")
}

#[test]
fn xmlreader_success_and_exception() -> anyhow::Result<()> {
    check_extension("php_ext/xmlreader-worker.php", "xr:a")
}

#[test]
fn xmlwriter_success_and_exception() -> anyhow::Result<()> {
    check_extension("php_ext/xmlwriter-worker.php", "xw:<v>ok</v>")
}

#[test]
fn pdo_sqlite_success_and_exception() -> anyhow::Result<()> {
    check_extension("php_ext/pdo_sqlite-worker.php", "pdo:ok")
}

#[test]
fn sqlite3_success_and_exception() -> anyhow::Result<()> {
    check_extension("php_ext/sqlite3-worker.php", "sqlite:42")
}

#[test]
fn filter_success_and_exception() -> anyhow::Result<()> {
    check_extension("php_ext/filter-worker.php", "filter:a@b.com")
}

/// Verifies that OPcache starts with this SAPI name. PHP 8.4 and earlier require the name in an allowlist. See build_sapi_module.
#[test]
fn opcache_success() -> anyhow::Result<()> {
    let name = "php_ext/opcache-worker.php";
    let out = run_worker(name, &["/"])?;
    if out[0].1 == "skip" {
        assert_skip_allowed(name);
        return Ok(());
    }
    assert_eq!(out[0].0, 200);
    assert_eq!(out[0].1, "opcache:enabled");
    Ok(())
}

/// ext/openssl has no RINIT or RSHUTDOWN in openssl.c for php-fpm or Rapira. Therefore, an unread error in the persistent ring remains after its request.
#[test]
fn openssl_error_ring_outlives_the_request() -> anyhow::Result<()> {
    // The first read removes errors that earlier tests added to the process-global ring.
    let out = run_worker(
        "php_ext/openssl-worker.php",
        &[
            "/?step=drain",
            "/?step=leak",
            "/?step=drain",
            "/?step=drain",
        ],
    )?;
    if out[0].1 == "skip" {
        assert_skip_allowed("php_ext/openssl-worker.php");
        return Ok(());
    }
    assert_eq!(
        (out[1].0, out[1].1.as_str()),
        (200, "openssl:leaked"),
        "leak request must succeed (got: {:?})",
        out[1]
    );
    // The PEM reader adds one PEM_R_NO_START_LINE entry. Only the routine and reason substrings are stable between OpenSSL 1.1.1 and 3.x.
    assert!(
        out[2].1.starts_with("openssl:drained:1:"),
        "the next request must drain exactly one error (got: {:?})",
        out[2].1
    );
    assert!(
        out[2].1.contains("PEM routines") && out[2].1.contains("no start line"),
        "drained error must be the PEM no-start-line entry (got: {:?})",
        out[2].1
    );
    assert_eq!(
        (out[3].0, out[3].1.as_str()),
        (200, "openssl:drained:0:"),
        "the ring is FIFO-drained, so the last request must find it empty (got: {:?})",
        out[3]
    );
    Ok(())
}

/// The 16-entry ring overwrites the oldest entry when full, so php_openssl_store_errors in openssl.c permits reading at most the newest 15 errors.
#[test]
fn openssl_error_ring_overwrites_the_oldest() -> anyhow::Result<()> {
    let out = run_worker(
        "php_ext/openssl-worker.php",
        &["/?step=drain", "/?step=leak_many", "/?step=drain"],
    )?;
    if out[0].1 == "skip" {
        assert_skip_allowed("php_ext/openssl-worker.php");
        return Ok(());
    }
    assert_eq!(
        (out[1].0, out[1].1.as_str()),
        (200, "openssl:leaked:20"),
        "the leak request must push 20 errors (got: {:?})",
        out[1]
    );
    assert!(
        out[2].1.starts_with("openssl:drained:15:"),
        "20 pushes wrap the 16-slot ring down to 15 readable entries (got: {:?})",
        out[2].1
    );
    Ok(())
}

#[test]
fn browscap_unset_success_and_exception() -> anyhow::Result<()> {
    check_extension("php_ext/browscap-worker.php", "browscap:false")
}
/// The `browscap` setting is PHP_INI_SYSTEM and is not set here. get_browser() must produce a warning and return false, as defined in browscap.c.
#[test]
fn browscap_unset_warns() -> anyhow::Result<()> {
    let out = run_worker("php_ext/browscap-worker.php", &["/"])?;
    if out[0].1 == "skip" {
        return Ok(());
    }
    assert_eq!(out[0].0, 200, "must serve 200 (got: {:?})", out[0]);
    assert!(
        out[0]
            .1
            .contains("get_browser(): browscap ini directive not set"),
        "the E_WARNING must be visible in the body (got: {:?})",
        out[0].1
    );
    assert!(
        out[0].1.contains("browscap:false"),
        "get_browser() must return false (got: {:?})",
        out[0].1
    );
    Ok(())
}

#[macro_use]
mod macros;

use std::env;
use std::path::{Path, PathBuf};

use anyhow::Context;

const ALLOWED_BINDINGS: &[&str] = include!("allowed_bindings.rs");

struct PhpBuild {
    /// include directories (no `-I` prefix; may contain spaces on Windows).
    includes: Vec<String>,
    /// directory to search for the PHP import library.
    lib_dir: String,
    /// the PHP import library name, e.g. `php8ts` / `php8ts_debug`.
    lib_name: String,
    abi: PhpAbi,
}

struct PhpAbi {
    version: (u32, u32),
    debug: bool,
}

fn main() -> anyhow::Result<()> {
    // php_zts is always set (this build is ZTS-only); php85 is version-gated.
    println!("cargo:rustc-check-cfg=cfg(php85, php_zts)");
    println!("cargo:rerun-if-env-changed=PHP_DEVEL_DIR");
    println!("cargo:rerun-if-env-changed=PHP_SDK_PATH");
    println!("cargo:rerun-if-env-changed=LIBCLANG_PATH"); // bindgen/libclang discovery

    let php = discover_php()?;

    println!("cargo:rustc-link-search=native={}", php.lib_dir);
    println!("cargo:rustc-link-lib=dylib={}", php.lib_name);

    // ZTS is mandatory here; emit the cfg unconditionally so any residual #[cfg(php_zts)]
    // stays live, and gate php85 on the devel pack's version.
    println!("cargo:rustc-cfg=php_zts");
    if php.abi.version >= (8, 5) {
        println!("cargo:rustc-cfg=php85");
    }

    let win_defs: Vec<(&str, &str)> = windows_defines(&php.abi);

    // --- C shim: wrapper.c + module.c ---
    let mut c = cc::Build::new();
    c.file("wrapper.c").file("module.c");
    c.define("ZTS", None); // ZTS-only build
    for &(k, v) in &win_defs {
        c.define(k, Some(v));
    }
    for d in &php.includes {
        c.include(d);
    }
    // Always /MD (static_crt(false)): rustc links only the release CRT on *-msvc, and /MDd
    // would define _DEBUG, turning zend_config.w32.h's _CRTDBG_MAP_ALLOC into free -> _free_dbg
    // references that can't resolve against msvcrt. Layout parity with a --enable-debug DLL
    // comes from the ZEND_DEBUG define, not the CRT; the DLL keeps its own debug CRT, which is
    // fine as long as CRT allocations don't cross the DLL boundary (ZMM calls run inside it).
    // https://doc.rust-lang.org/reference/linkage.html#static-and-dynamic-c-runtimes
    // https://learn.microsoft.com/en-us/cpp/build/reference/md-mt-ld-use-run-time-library
    c.static_crt(false);
    c.debug(php.abi.debug); // debug info only; does not select /MDd
    c.compile("rapira_shim");

    // --- bindgen ---
    let mut bindings = bindgen::Builder::default()
        .header("wrapper.h")
        .clang_args(php.includes.iter().map(|d| format!("-I{d}")))
        .clang_args(win_defs.iter().map(|(k, v)| format!("-D{k}={v}")))
        .clang_arg("-DZTS")
        // Parse-only marker for wrapper.h's Windows rewrites (overflow builtins + blanking
        // ZEND_FASTCALL so the __vectorcall handler field renders as a plain 8-byte pointer,
        // keeping layout_tests exact). The real cl.exe never sees it.
        .clang_arg("-DRAPIRA_BINDGEN=1")
        // php-src master on clang >=19 makes zend_op.handler a `preserve_none` pointer, which
        // bindgen 0.72.1 can't emit and panics on. opaque_type renders _zend_op as a byte array
        // of clang's reported size/align, skipping the handler field; rapira reads no _zend_op
        // field, so nothing is lost (no-op on 8.4/8.5).
        // https://clang.llvm.org/docs/AttributeReference.html#preserve-none
        .opaque_type("_zend_op")
        .layout_tests(true);

    for binding in ALLOWED_BINDINGS {
        bindings = bindings
            .allowlist_function(binding)
            .allowlist_type(binding)
            .allowlist_var(binding);
    }

    bindings
        .generate()?
        .write_to_file(PathBuf::from(env::var("OUT_DIR")?).join("bindings.rs"))?;

    for f in ["wrapper.h", "module.c", "wrapper.c", "allowed_bindings.rs"] {
        println!("cargo:rerun-if-changed={f}");
    }

    Ok(())
}

// PHP passes these on the compiler command line, never in a header; without ZEND_WIN32 the
// headers #include the Unix-only <zend_config.h> and the build dies.
fn windows_defines(abi: &PhpAbi) -> Vec<(&'static str, &'static str)> {
    vec![
        ("ZEND_WIN32", "1"),
        ("PHP_WIN32", "1"),
        ("WIN32", "1"),
        ("WINDOWS", "1"),
        ("_WINDOWS", "1"),
        ("_MBCS", "1"),
        ("_USE_MATH_DEFINES", "1"),
        // ZEND_DEBUG is referenced unconditionally (STANDARD_MODULE_HEADER builds the module
        // struct from it) and on Windows is only ever a command-line define: 1 to match a
        // --enable-debug DLL's struct layout, else 0.
        ("ZEND_DEBUG", if abi.debug { "1" } else { "0" }),
    ]
}

fn parse_version(v: &str) -> anyhow::Result<(u32, u32)> {
    let mut it = v.trim().split('.');
    let major: u32 = it.next().context("php version missing major")?.parse()?;
    let minor: u32 = it.next().context("php version missing minor")?.parse()?;
    Ok((major, minor))
}

// PHP_DEVEL_DIR (fallback PHP_SDK_PATH) points at the extracted devel pack root:
//   {root}\include\{,main,Zend,TSRM,ext,win32}, {root}\lib\php{major}ts[_debug].lib
fn discover_php() -> anyhow::Result<PhpBuild> {
    let root = env::var("PHP_DEVEL_DIR")
        .or_else(|_| env::var("PHP_SDK_PATH"))
        .context("set PHP_DEVEL_DIR to the extracted PHP devel pack root")?;
    let root = PathBuf::from(root);
    let inc = root.join("include");
    let include_dirs = ["", "main", "Zend", "TSRM", "ext", "win32"];
    let includes: Vec<String> = include_dirs
        .iter()
        .map(|d| inc.join(d).display().to_string())
        .collect();
    let lib_dir = root.join("lib");
    let version = parse_version(&read_php_version(&inc)?)?;
    let major = version.0;

    // The devel pack's import libs are the ground truth for the ABI being linked. This build is
    // ZTS-only: require php{major}ts.lib (or its _debug variant) and hard-error otherwise.
    let has_lib = |suffix: &str| lib_dir.join(format!("php{major}{suffix}.lib")).exists();
    let debug = has_lib("ts_debug");
    anyhow::ensure!(
        debug || has_lib("ts"),
        "no ZTS PHP import lib (php{major}ts.lib / php{major}ts_debug.lib) in {}; \
         rapira-windows is ZTS-only",
        lib_dir.display()
    );
    let lib_name = windows_lib_name(&lib_dir, major)?;
    Ok(PhpBuild {
        includes,
        lib_dir: lib_dir.display().to_string(),
        lib_name,
        abi: PhpAbi { version, debug },
    })
}

// Prefer the `_debug` import lib when the devel pack shipped one, else the release ts lib.
fn windows_lib_name(lib_dir: &Path, major: u32) -> anyhow::Result<String> {
    for stem in [format!("php{major}ts_debug"), format!("php{major}ts")] {
        if lib_dir.join(format!("{stem}.lib")).exists() {
            return Ok(stem);
        }
    }
    anyhow::bail!(
        "no ZTS PHP import lib (php{major}ts_debug.lib / php{major}ts.lib) in {}",
        lib_dir.display()
    )
}

fn read_php_version(include: &Path) -> anyhow::Result<String> {
    let header = include.join("main").join("php_version.h");
    let text = std::fs::read_to_string(&header)
        .with_context(|| format!("reading {}", header.display()))?;
    let grab = |name: &str| {
        let needle = format!("#define {name} ");
        text.lines()
            .find_map(|l| l.strip_prefix(needle.as_str()))
            .map(|v| v.trim().to_string())
    };
    let major = grab("PHP_MAJOR_VERSION").context("PHP_MAJOR_VERSION not found")?;
    let minor = grab("PHP_MINOR_VERSION").context("PHP_MINOR_VERSION not found")?;
    Ok(format!("{major}.{minor}"))
}

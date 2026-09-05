#[macro_use]
mod macros;

use std::env;
use std::path::{Path, PathBuf};

use anyhow::Context;

const ALLOWED_BINDINGS: &[&str] = include!("allowed_bindings.rs");

#[derive(Debug)]
struct WindowsLinks;

impl bindgen::callbacks::ParseCallbacks for WindowsLinks {
    fn generated_link_name_override(
        &self,
        item: bindgen::callbacks::ItemInfo<'_>,
    ) -> Option<String> {
        match item.name {
            "sapi_startup"
            | "zend_hash_internal_pointer_reset_ex"
            | "zend_hash_get_current_data_ex"
            | "zend_hash_get_current_key_ex"
            | "zend_hash_move_forward_ex"
            | "zend_hash_index_update"
            | "zend_hash_str_update"
            | "instanceof_function_slow" => Some(format!("rapira_{}", item.name)),
            _ => None,
        }
    }
}

struct PhpBuild {
    /// Include directories without the `-I` prefix. Paths can contain spaces on Windows.
    includes: Vec<String>,
    /// Directory that contains the PHP import library.
    lib_dir: String,
    /// PHP import library name, such as `php8ts` or `php8ts_debug`.
    lib_name: String,
    abi: PhpAbi,
}

struct PhpAbi {
    version: (u32, u32),
    debug: bool,
}

fn main() -> anyhow::Result<()> {
    println!("cargo:rustc-check-cfg=cfg(php84, php85)");
    println!("cargo:rerun-if-env-changed=PHP_DEVEL_DIR");
    println!("cargo:rerun-if-env-changed=PHP_SDK_PATH");
    println!("cargo:rerun-if-env-changed=LIBCLANG_PATH"); // Locates bindgen and libclang.

    let php = discover_php()?;

    println!("cargo:rustc-link-search=native={}", php.lib_dir);
    println!("cargo:rustc-link-lib=dylib={}", php.lib_name);

    if php.abi.version >= (8, 5) {
        println!("cargo:rustc-cfg=php85");
    } else {
        println!("cargo:rustc-cfg=php84");
    }

    let win_defs: Vec<(&str, &str)> = windows_defines(&php.abi);

    let version = env::var("CARGO_PKG_VERSION").expect("cargo sets CARGO_PKG_VERSION");
    let mut c = cc::Build::new();
    c.define("RAPIRA_VERSION", format!("\"{version}\"").as_str());
    c.file("wrapper.c")
        .file("module.c")
        .file("rapira_classes.c")
        .file("rapira_http.c")
        .file("rapira_dispatcher.c")
        .file("rapira_exchange.c");
    c.define("ZTS", None); // Builds only for ZTS.
    for &(k, v) in &win_defs {
        c.define(k, Some(v));
    }
    for d in &php.includes {
        c.include(d);
    }
    // Always use /MD through static_crt(false). rustc links only the release CRT on *-msvc. /MDd defines _DEBUG, which changes free references to _free_dbg through _CRTDBG_MAP_ALLOC in zend_config.w32.h. msvcrt cannot resolve these references. ZEND_DEBUG gives the required layout for a --enable-debug DLL. CRT allocations do not cross the DLL boundary because ZMM calls run inside the DLL.
    // https://doc.rust-lang.org/reference/linkage.html#static-and-dynamic-c-runtimes
    // https://learn.microsoft.com/en-us/cpp/build/reference/md-mt-ld-use-run-time-library
    c.static_crt(false);
    c.debug(php.abi.debug); // Adds debug information but does not select /MDd.
    c.compile("rapira_shim");

    let mut bindings = bindgen::Builder::default()
        .header("wrapper.h")
        .parse_callbacks(Box::new(WindowsLinks))
        // Bindgen does not emit link-name overrides for extern variables.
        .blocklist_var("zend_string_init_interned")
        .raw_line("pub use self::rapira_zend_string_init_interned as zend_string_init_interned;")
        .blocklist_var("zend_ce_throwable")
        .raw_line(format!(
            "#[link(name = {:?}, kind = \"dylib\")] unsafe extern \"C\" {{ pub static mut zend_ce_throwable: *mut zend_class_entry; }}",
            php.lib_name
        ))
        .clang_args(php.includes.iter().map(|d| format!("-I{d}")))
        .clang_args(win_defs.iter().map(|(k, v)| format!("-D{k}={v}")))
        .clang_arg("-DZTS")
        // This parse-only marker enables the Windows substitutions in wrapper.h. The substitutions provide overflow builtins and remove ZEND_FASTCALL so bindgen represents the __vectorcall handler field as an 8-byte pointer. This preserves the layout tests. cl.exe does not receive this marker.
        .clang_arg("-DRAPIRA_BINDGEN=1")
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

    for f in [
        "wrapper.h",
        "module.c",
        "wrapper.c",
        "allowed_bindings.rs",
        "rapira_classes.c",
        "rapira_classes.h",
        "rapira_http.c",
        "rapira_dispatcher.c",
        "rapira_exchange.c",
        "rapira.stub.php",
        "rapira_arginfo.h",
        "rapira_http.stub.php",
        "rapira_http_arginfo.h",
        "rapira_exception.stub.php",
        "rapira_exception_arginfo.h",
    ] {
        println!("cargo:rerun-if-changed={f}");
    }

    Ok(())
}

// PHP defines these values only on the compiler command line. Without ZEND_WIN32, the headers include the Unix-only zend_config.h file and compilation fails.
fn windows_defines(abi: &PhpAbi) -> Vec<(&'static str, &'static str)> {
    vec![
        ("ZEND_WIN32", "1"),
        ("PHP_WIN32", "1"),
        ("WIN32", "1"),
        ("WINDOWS", "1"),
        ("_WINDOWS", "1"),
        ("_MBCS", "1"),
        ("_USE_MATH_DEFINES", "1"),
        // STANDARD_MODULE_HEADER always uses ZEND_DEBUG to build the module structure. On Windows, the compiler command defines it as 1 for the structure layout of a --enable-debug DLL and 0 otherwise.
        ("ZEND_DEBUG", if abi.debug { "1" } else { "0" }),
    ]
}

// PHP_DEVEL_DIR, or PHP_SDK_PATH as a fallback, identifies the extracted development package root:
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
    let version = read_php_version(&inc)?;
    let major = version.0;

    // The import libraries in the development package define the linked ABI. This build requires php{major}ts.lib or its _debug variant because it supports only ZTS.
    let has_lib = |suffix: &str| lib_dir.join(format!("php{major}{suffix}.lib")).exists();
    let debug = has_lib("ts_debug");
    anyhow::ensure!(
        debug || has_lib("ts"),
        "no ZTS PHP import lib (php{major}ts.lib / php{major}ts_debug.lib) in {}; \
         rapira-windows is ZTS-only",
        lib_dir.display()
    );
    let lib_name = format!("php{major}ts{}", if debug { "_debug" } else { "" });
    Ok(PhpBuild {
        includes,
        lib_dir: lib_dir.display().to_string(),
        lib_name,
        abi: PhpAbi { version, debug },
    })
}

fn read_php_version(include: &Path) -> anyhow::Result<(u32, u32)> {
    let header = include.join("main").join("php_version.h");
    let text = std::fs::read_to_string(&header)
        .with_context(|| format!("reading {}", header.display()))?;
    let grab = |name: &str| {
        let needle = format!("#define {name} ");
        text.lines()
            .find_map(|l| l.strip_prefix(needle.as_str()))
            .map(str::trim)
    };
    let major = grab("PHP_MAJOR_VERSION").context("PHP_MAJOR_VERSION not found")?;
    let minor = grab("PHP_MINOR_VERSION").context("PHP_MINOR_VERSION not found")?;
    Ok((major.parse()?, minor.parse()?))
}

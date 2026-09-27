use std::env;
use std::path::PathBuf;

use anyhow::Context;

/// Native ZTS headers and import library shared by the SAPI and plugin builds.
pub struct Php {
    pub includes: Vec<String>,
    pub lib_dir: PathBuf,
    pub lib_name: String,
    pub version: (u32, u32),
    pub debug: bool,
}

pub fn discover() -> anyhow::Result<Php> {
    let root = env::var("PHP_DEVEL_DIR")
        .context("set PHP_DEVEL_DIR to the native PHP development directory")?;
    let root = PathBuf::from(root);
    let include = root.join("include");
    let text = std::fs::read_to_string(include.join("main/php_version.h"))?;
    let number = |name: &str| -> anyhow::Result<u32> {
        let prefix = format!("#define {name} ");
        Ok(text
            .lines()
            .find_map(|line| line.strip_prefix(&prefix))
            .with_context(|| format!("missing {name} in php_version.h"))?
            .trim()
            .parse()?)
    };
    let version = (number("PHP_MAJOR_VERSION")?, number("PHP_MINOR_VERSION")?);
    anyhow::ensure!(matches!(version, (8, 4 | 5)), "PHP 8.4 or 8.5 is required");
    let lib_dir = root.join("lib");
    let debug = lib_dir.join("php8ts_debug.lib").is_file();
    let lib_name = if debug { "php8ts_debug" } else { "php8ts" }.to_string();
    anyhow::ensure!(
        lib_dir.join(format!("{lib_name}.lib")).is_file(),
        "no ZTS PHP import library in {}",
        lib_dir.display()
    );
    let config = std::fs::read_to_string(include.join("main/config.w32.h"))?;
    let target = env::var("CARGO_CFG_TARGET_ARCH")?;
    let arch = match target.as_str() {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        _ => anyhow::bail!("unsupported Windows architecture: {target}"),
    };
    anyhow::ensure!(
        config
            .lines()
            .any(|line| line.trim() == format!("#define PHP_BUILD_ARCH \"{arch}\"")),
        "PHP development files do not match {target}"
    );
    Ok(Php {
        includes: ["", "main", "Zend", "TSRM", "ext", "win32"]
            .iter()
            .map(|dir| include.join(dir).display().to_string())
            .collect(),
        lib_dir,
        lib_name,
        version,
        debug,
    })
}

impl Php {
    pub fn defines(&self) -> Vec<(&'static str, &'static str)> {
        vec![
            ("ZTS", "1"),
            ("ZEND_WIN32", "1"),
            ("PHP_WIN32", "1"),
            ("WIN32", "1"),
            ("WINDOWS", "1"),
            ("_WINDOWS", "1"),
            ("_MBCS", "1"),
            ("_USE_MATH_DEFINES", "1"),
            ("ZEND_DEBUG", if self.debug { "1" } else { "0" }),
        ]
    }
}

/// Compiles a crate's C method shells with the same PHP ABI as the SAPI.
pub fn compile(name: &str, files: &[&str], php: &Php, extra_includes: &[&str]) {
    let version = env::var("CARGO_PKG_VERSION").expect("cargo sets CARGO_PKG_VERSION");
    let mut c = cc::Build::new();
    c.define("RAPIRA_VERSION", format!("\"{version}\"").as_str());
    for (name, value) in php.defines() {
        c.define(name, value);
    }
    // Rust uses the release CRT on MSVC. Zend allocations stay inside the PHP DLL.
    // https://doc.rust-lang.org/reference/linkage.html#static-and-dynamic-c-runtimes
    c.static_crt(false).debug(php.debug);
    c.files(files)
        .includes(&php.includes)
        .includes(extra_includes)
        .compile(name);
    for file in files {
        println!("cargo:rerun-if-changed={file}");
    }
}

pub fn rerun_if_changed(files: &[&str]) {
    for name in ["PATH", "PHP_DEVEL_DIR", "LIBCLANG_PATH", "CLANG_PATH"] {
        println!("cargo:rerun-if-env-changed={name}");
    }
    for file in files {
        println!("cargo:rerun-if-changed={file}");
    }
}

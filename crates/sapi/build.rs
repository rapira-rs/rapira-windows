use std::env;
use std::path::PathBuf;

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
            | "zend_hash_str_find"
            | "zend_hash_extend"
            | "instanceof_function_slow" => Some(format!("rapira_{}", item.name)),
            _ => None,
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rustc-check-cfg=cfg(php84, php85)");
    let php = rapira_php_build::discover()?;
    println!("cargo:rustc-link-search=native={}", php.lib_dir.display());
    println!("cargo:rustc-link-lib=dylib={}", php.lib_name);
    println!(
        "cargo:rustc-cfg={}",
        if php.version >= (8, 5) {
            "php85"
        } else {
            "php84"
        }
    );
    rapira_php_build::compile(
        "rapira_sapi",
        &[
            "wrapper.c",
            "module.c",
            "rapira_classes.c",
            "rapira_dispatcher.c",
        ],
        &php,
        &[],
    );

    let mut bindings = bindgen::Builder::default()
        .header("rapira_sapi.h")
        .parse_callbacks(Box::new(WindowsLinks))
        .blocklist_var("zend_string_init_interned")
        .raw_line("pub use self::rapira_zend_string_init_interned as zend_string_init_interned;")
        .blocklist_var("zend_ce_throwable")
        .raw_line(format!(
            "#[link(name = {:?}, kind = \"dylib\")] unsafe extern \"C\" {{ pub static mut zend_ce_throwable: *mut zend_class_entry; }}",
            php.lib_name))
        .blocklist_var("php_import_environment_variables")
        .raw_line(format!(
            "#[link(name = {:?}, kind = \"dylib\")] unsafe extern \"C\" {{ pub static mut php_import_environment_variables: Option<unsafe extern \"C\" fn(*mut zval)>; }}",
            php.lib_name))
        .clang_args(php.includes.iter().map(|d| format!("-I{d}")))
        .clang_args(php.defines().iter().map(|(k, v)| format!("-D{k}={v}")))
        .clang_arg("-DRAPIRA_BINDGEN=1")
        .opaque_type("_zend_op");
    for binding in ALLOWED_BINDINGS {
        bindings = bindings.allowlist_item(binding);
    }
    bindings
        .generate()?
        .write_to_file(PathBuf::from(env::var("OUT_DIR")?).join("bindings.rs"))?;
    println!("cargo:include={}", env::var("CARGO_MANIFEST_DIR")?);
    rapira_php_build::rerun_if_changed(&[
        "rapira_sapi.h",
        "allowed_bindings.rs",
        "rapira_arginfo.h",
        "rapira_exception_arginfo.h",
    ]);
    Ok(())
}

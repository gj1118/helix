use std::path::Path;
use std::{env, fs};

fn main() {
    if env::var_os("DISABLED_TS_BUILD").is_some() {
        return;
    }
    let mut config = cc::Build::new();

    let manifest_path = Path::new(env!("CARGO_MANIFEST_DIR"));
    let include_path = manifest_path.join("vendor/include");
    let src_path = manifest_path.join("vendor/src");
    for entry in fs::read_dir(&src_path).unwrap() {
        let entry = entry.unwrap();
        let path = src_path.join(entry.file_name());
        println!("cargo:rerun-if-changed={}", path.to_str().unwrap());
    }

    // PATCH (helix fork): tree-sitter is optimized in debug builds too, where it otherwise
    // dominates the time of every test touching syntax.
    if env::var("OPT_LEVEL").as_deref() == Ok("0") {
        config.opt_level(2);
    }
    config
        .flag_if_supported("-std=c11")
        .flag_if_supported("-fvisibility=hidden")
        .flag_if_supported("-Wshadow")
        .flag_if_supported("-Wno-unused-parameter")
        .flag_if_supported("-Wno-incompatible-pointer-types")
        .include(&src_path)
        .include(&include_path)
        .define("_POSIX_C_SOURCE", "200112L")
        .define("_DEFAULT_SOURCE", None)
        .warnings(false)
        .file(src_path.join("lib.c"))
        .compile("tree-sitter");
}

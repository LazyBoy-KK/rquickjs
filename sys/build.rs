use std::{env, fs, path::Path};

fn main() {
    #[cfg(feature = "logging")]
    pretty_env_logger::init();

    println!("cargo:rerun-if-changed=build.rs");

    let features = [
        "exports",
        "bindgen",
        "update-bindings",
        "dump-bytecode",
        "dump-gc",
        "dump-gc-free",
        "dump-free",
        "dump-leaks",
        "dump-mem",
        "dump-objects",
        "dump-atoms",
        "dump-shapes",
        "dump-module-resolve",
        "dump-promise",
        "dump-read-object",
    ];

    for feature in &features {
        println!("cargo:rerun-if-env-changed={}", feature_to_cargo(feature));
    }

    let src_dir = Path::new("quickjs");

    let out_dir = env::var("OUT_DIR").expect("No OUT_DIR env var is set by cargo");
    let out_dir = Path::new(&out_dir);

    let header_files = [
        "libbf.h",
        "libregexp-opcode.h",
        "libregexp.h",
        "libunicode-table.h",
        "libunicode.h",
        "list.h",
        "quickjs-atom.h",
        "quickjs-opcode.h",
        "quickjs-libc.h",
        "quickjs.h",
        "cutils.h",
    ];

    let source_files = [
        "libregexp.c",
        "libunicode.c",
        "cutils.c",
        "quickjs-libc.c",
        "quickjs.c",
        "libbf.c",
    ];

    let mut defines = vec![
        ("_GNU_SOURCE".into(), None),
        ("CONFIG_VERSION".into(), Some("\"2020-01-19\"")),
        ("CONFIG_BIGNUM".into(), None),
    ];

    if env::var("CARGO_FEATURE_QUICKJS_LIBC").is_ok() {
        defines.push(("CONFIG_WASM".into(), None));
		if env::var("CARGO_FEATURE_ALLOCATOR").is_ok() {
			defines.push(("CONFIG_CUSTOM_ALLOCATOR".into(), None));
		}
    }

	// defines.push(("CONFIG_BENCHMARK".into(), None));

    for feature in &features {
        if feature.starts_with("dump-") && env::var(feature_to_cargo(feature)).is_ok() {
            defines.push((feature_to_define(feature), None));
        }
    }

    for file in source_files.iter().chain(header_files.iter()) {
        fs::copy(src_dir.join(file), out_dir.join(file)).expect("Unable to copy source");
    }

    // generating bindings
    bindgen(out_dir, src_dir.join("quickjs.h"), &defines);
    bindgen(out_dir, src_dir.join("quickjs-libc.h"), &defines);
    let mut builder = cc::Build::new();
    builder
        .extra_warnings(false)
        //.flag("-Wno-array-bounds")
        //.flag("-Wno-format-truncation")
        ;

    for (name, value) in &defines {
        builder.define(name, *value);
    }

    for src in &source_files {
        builder.file(out_dir.join(src));
    }

    builder
		.compiler("clang-18")
		.compile("libquickjs.a");
}

fn feature_to_cargo(name: impl AsRef<str>) -> String {
    format!("CARGO_FEATURE_{}", feature_to_define(name))
}

fn feature_to_define(name: impl AsRef<str>) -> String {
    name.as_ref().to_uppercase().replace('-', "_")
}

#[cfg(not(feature = "bindgen"))]
fn bindgen<'a, D, H, X, K, V>(out_dir: D, _header_file: H, _defines: X)
where
    D: AsRef<Path>,
    H: AsRef<Path>,
    X: IntoIterator<Item = &'a (K, Option<V>)>,
    K: AsRef<str> + 'a,
    V: AsRef<str> + 'a,
{
    let target = env::var("TARGET").unwrap();

    let bindings_file = out_dir.as_ref().join("bindings.rs");

    fs::write(
        bindings_file,
        format!(
            r#"macro_rules! bindings_env {{
                ("TARGET") => {{ "{target}" }};
            }}"#
        ),
    )
    .unwrap();
}

#[cfg(feature = "bindgen")]
fn bindgen<'a, D, H, X, K, V>(out_dir: D, header_file: H, defines: X)
where
    D: AsRef<Path>,
    H: AsRef<Path>,
    X: IntoIterator<Item = &'a (K, Option<V>)>,
    K: AsRef<str> + 'a,
    V: AsRef<str> + 'a,
{
    let target = env::var("TARGET").unwrap();
    let out_dir = out_dir.as_ref();
    let header_file = header_file.as_ref();

    let mut cflags = vec![format!("--target={}", target.clone())];

    //format!("-I{}", out_dir.parent().display())

    for (name, value) in defines {
        cflags.push(if let Some(value) = value {
            format!("-D{}={}", name.as_ref(), value.as_ref())
        } else {
            format!("-D{}", name.as_ref())
        });
    }
    if target == "aarch64-linux-android" {
        cflags.push("-DANDROID_ARM64V8A".to_owned());
    }

    let bindings = bindgen_rs::Builder::default()
        .detect_include_paths(true)
        .clang_arg("-xc")
        .clang_arg("-v")
        .clang_args(cflags)
        .header(header_file.display().to_string())
        .allowlist_type("JS.*")
        .allowlist_function("js.*")
        .allowlist_function("JS.*")
        .allowlist_function("__JS.*")
        .allowlist_var("JS.*")
        .opaque_type("FILE")
        .blocklist_type("FILE")
        .blocklist_function("JS_DumpMemoryUsage")
        .size_t_is_usize(false)
        .generate()
        .expect("Unable to generate bindings");

    let bindings_file = out_dir.join("bindings.rs");

    bindings
        .write_to_file(&bindings_file)
        .expect("Couldn't write bindings");

    // Special case to support bundled bindings
    if env::var("CARGO_FEATURE_UPDATE_BINDINGS").is_ok() {
        let dest_dir = Path::new("src").join("bindings");
        fs::create_dir_all(&dest_dir).unwrap();

        let dest_file = format!("{}.rs", target);
        fs::copy(&bindings_file, dest_dir.join(&dest_file)).unwrap();
    }
}

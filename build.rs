use std::io::Result;
use std::path::{Path, PathBuf};

fn main() -> Result<()> {
    // Generate protobuf bindings
    let empty_array: &[&Path] = &[];
    println!("cargo:rerun-if-changed=protos/messages.proto");
    prost_build::compile_protos(&["protos/messages.proto"], empty_array)?;
    println!("cargo:rerun-if-changed=protos/vm.proto");
    prost_build::compile_protos(&["protos/vm.proto"], empty_array)?;

    if let Ok(target_os) = std::env::var("CARGO_CFG_TARGET_OS") {
        if target_os == "android" {
            setup_android_environment()
        }
    }

    // When using the library directly from Rust (e.g. in WASM) there is no need for C bindings
    // It may also cause compile failures as "stddef.h" not found etc. in wasm32 toolchain
    #[cfg(feature="c_bindings")]
    {
        // The bindgen::Builder is the main entry point
        // to bindgen, and lets you build up options for
        // the resulting bindings.
        let bindings = bindgen::Builder::default()
            // The input header we would like to generate
            // bindings for.
            .header("include/graph_witness.h")
            .allowlist_type("gw_status_t")
            // Tell cargo to invalidate the built crate whenever any of the
            // included header files changed.
            .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
            // Finish the builder and generate the bindings.
            .generate()
            // Unwrap the Result and panic on failure.
            .expect("Unable to generate bindings");

        // Write the bindings to the $OUT_DIR/bindings.rs file.
        let out_path = PathBuf::from(std::env::var("OUT_DIR").unwrap());
        bindings
            .write_to_file(out_path.join("bindings.rs"))
            .expect("Couldn't write bindings!");
    }

    Ok(())
}

fn setup_android_environment() {

    // maybe this is useful for build on Linux, need to check someday.
    // println!("cargo:rustc-link-arg=-Wl,-soname,libcircom_witnesscalc.so");

    println!("cargo:rustc-cdylib-link-arg=-Wl,-soname,libcircom_witnesscalc.so");

    let target = std::env::var("TARGET")
        .expect("TARGET environment variable is not set");

    let linker_env = get_linker_env_var(&target);

    let cc_target = format!("CC_{}", target);
    // println!("cargo:warning=Looking for target-specific compiler: {}", cc_target);

    if let Ok(clang_path) = std::env::var(&cc_target) {
        // println!("cargo:warning=Found target compiler: {}", clang_path);
        std::env::set_var("CC", &clang_path);
        std::env::set_var("CLANG_PATH", &clang_path);
        return;
    }

    let cc_missing = std::env::var("CC").is_err();
    let clang_path_missing = std::env::var("CLANG_PATH").is_err();
    let linker_missing = std::env::var(&linker_env).is_err();

    if cc_missing || clang_path_missing || linker_missing {
        let mut missing_vars = Vec::new();
        if cc_missing { missing_vars.push("CC"); }
        if clang_path_missing { missing_vars.push("CLANG_PATH"); }
        if linker_missing { missing_vars.push(&linker_env); }

        let missing = missing_vars.join(" and ");

        panic!(
            "Android build requires proper compiler configuration.\n\
             {} environment variable(s) not set.\n\
             Please run with cargo-ndk or set these environment variables manually.",
            missing
        );
    }

    // println!("cargo:warning=Using existing CC and CLANG_PATH environment variables");
}

fn get_linker_env_var(target: &str) -> String {
    let upper_target = target.to_uppercase();
    let formatted_target = upper_target.replace('-', "_");
    format!("CARGO_TARGET_{}_LINKER", formatted_target)
}

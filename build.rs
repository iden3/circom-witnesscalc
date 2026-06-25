use std::io::Result;
use std::path::Path;

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

    if let Ok(cc_path) = std::env::var(&cc_target) {
        // println!("cargo:warning=Found target compiler: {}", cc_path);
        std::env::set_var("CC", &cc_path);
        return;
    }

    let cc_missing = std::env::var("CC").is_err();
    let linker_missing = std::env::var(&linker_env).is_err();

    if cc_missing || linker_missing {
        let mut missing_vars = Vec::new();
        if cc_missing { missing_vars.push("CC"); }
        if linker_missing { missing_vars.push(&linker_env); }

        let missing = missing_vars.join(" and ");

        panic!(
            "Android build requires proper compiler configuration.\n\
             {} environment variable(s) not set.\n\
             Please run with cargo-ndk or set these environment variables manually.",
            missing
        );
    }

    // println!("cargo:warning=Using existing CC environment variable");
}

fn get_linker_env_var(target: &str) -> String {
    let upper_target = target.to_uppercase();
    let formatted_target = upper_target.replace('-', "_");
    format!("CARGO_TARGET_{}_LINKER", formatted_target)
}

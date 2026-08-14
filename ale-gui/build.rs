fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "android" {
        let config = slint_build::CompilerConfiguration::new().with_style("fluent".into());
        slint_build::compile_with_config("ui/android-app.slint", config).unwrap();
        println!("cargo:rerun-if-changed=android/res/");
        println!("cargo:rustc-link-lib=camera2ndk");
    } else {
        slint_build::compile("ui/app.slint").unwrap();
    }
}

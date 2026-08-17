fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("android") {
        let config = slint_build::CompilerConfiguration::new().with_style("fluent".into());
        slint_build::compile_with_config("ui/android-app.slint", config).unwrap();
        println!("cargo:rerun-if-changed=android/res/");
        println!("cargo:rustc-link-lib=camera2ndk");
    }
}

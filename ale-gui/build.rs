fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "android" {
        slint_build::compile("ui/android-app.slint").unwrap();
        println!("cargo:rerun-if-changed=android/res/");
        println!("cargo:rustc-link-lib=camera2ndk");
    } else {
        slint_build::compile("ui/app.slint").unwrap();
    }
}

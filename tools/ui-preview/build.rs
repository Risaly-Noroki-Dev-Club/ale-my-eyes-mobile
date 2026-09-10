fn main() {
    slint_build::compile_with_config(
        "../../ale-gui/ui/android-app.slint",
        slint_build::CompilerConfiguration::new().with_style("fluent".into()),
    )
    .unwrap();
}

fn main() {
    // Fluent's dark style for the few standard widgets we use (text fields,
    // scroll bars); everything else is drawn with our own theme.
    let config = slint_build::CompilerConfiguration::new().with_style("fluent-dark".into());
    if let Err(e) = slint_build::compile_with_config("ui/main.slint", config) {
        println!("cargo::error=ui/main.slint: {e}");
    }
    // The .exe's icon. `cfg(windows)` is the build machine (where
    // embed-resource exists); the target must be Windows too.
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo::rerun-if-changed=packaging/windows/hit-enter.rc");
        println!("cargo::rerun-if-changed=packaging/icons/icon.ico");
        if let Err(e) = embed_resource::compile("packaging/windows/hit-enter.rc", embed_resource::NONE)
            .manifest_optional()
        {
            println!("cargo::warning=the .exe has no icon: {e}");
        }
    }
}

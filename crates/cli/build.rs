fn main() {
    println!("cargo:rerun-if-changed=assets/solvela.ico");

    // Embed the Solvela icon (dark tile) + VERSIONINFO into solvela.exe.
    // The winresource dep is target-gated to cfg(windows) in Cargo.toml, so
    // this block must be compiled out on non-Windows hosts; the env check
    // guards the (unused) host=windows -> target=non-windows direction.
    #[cfg(target_os = "windows")]
    {
        if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
            let mut res = winresource::WindowsResource::new();
            res.set_icon("assets/solvela.ico");
            // ponytail: warn-and-continue — the icon is cosmetic and this path
            // only runs on release runners; failing here would brick a release
            // over a resource-compiler hiccup. The warning is visible in CI logs.
            if let Err(e) = res.compile() {
                println!("cargo:warning=failed to embed solvela.exe icon: {e}");
            }
        }
    }
}

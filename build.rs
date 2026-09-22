fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() == "windows" {
        let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
        if target_env == "gnu" {
            println!("cargo:rustc-link-arg=-Wl,--stack=23068672");
        } else {
            println!("cargo:rustc-link-arg=/STACK:23068672");
        }

        let mut res = winres::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        if let Ok(version) = std::env::var("CARGO_PKG_VERSION") {
            res.set("ProductName", "Sylphra");
            res.set("FileDescription", "Sylphra document browser");
            res.set("ProductVersion", &version);
            res.set("FileVersion", &version);
        }
        if let Err(error) = res.compile() {
            panic!("failed to compile Windows resources: {error}");
        }
        if target_env == "gnu" {
            let resource_object = std::path::PathBuf::from(
                std::env::var("OUT_DIR").expect("Cargo must set OUT_DIR for build scripts"),
            )
            .join("resource.o");
            println!("cargo:rustc-link-arg={}", resource_object.display());
        }
    }
}

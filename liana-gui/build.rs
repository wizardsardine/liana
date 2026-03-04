fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    // Skip icon if LIANA_SKIP_GUI_ICON env var is set (used by liana-business nix build)
    // or if the skip-windows-icon feature is enabled (used by liana-business cargo build)
    let skip_icon = std::env::var("LIANA_SKIP_GUI_ICON").is_ok()
        || std::env::var("CARGO_FEATURE_SKIP_WINDOWS_ICON").is_ok();

    if target_os == "windows" && !skip_icon {
        let out_dir = std::env::var("OUT_DIR").unwrap();

        let mut res = winresource::WindowsResource::new();
        res.set_icon("../liana-ui/static/logos/liana.ico");

        if let Ok(path) = std::env::var("TOOLKIT_x86_64_pc_windows_gnu") {
            res.set_toolkit_path(&path);
        }

        if let Ok(windres_path) = std::env::var("WINDRES_x86_64_pc_windows_gnu") {
            res.set_windres_path(&windres_path);
        }

        if let Ok(path) = std::env::var("AR_x86_64_pc_windows_gnu") {
            res.set_ar_path(&path);
        }

        if let Err(e) = res.compile() {
            eprintln!("Failed to compile Windows resources: {}", e);
            panic!("Windows resource compilation failed: {}", e);
        }

        // Explicitly link the resource object file (needed for cross-compilation)
        let resource_obj = format!("{}/resource.o", out_dir);
        if std::path::Path::new(&resource_obj).exists() {
            println!("cargo:rustc-link-arg-bins={}", resource_obj);
        }
    }
}

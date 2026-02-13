fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    // Skip icon if LIANA_SKIP_GUI_ICON env var is set (used by liana-business nix build)
    let skip_icon = std::env::var("LIANA_SKIP_GUI_ICON").is_ok();

    if target_os == "windows" && !skip_icon {
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
    }
}

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "windows" {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../liana-ui/static/logos/liana.ico");

        if let Ok(path) = std::env::var("TOOLKIT_x86_64_pc_windows_gnu") {
            println!("cargo:warning=Using toolkit at: {}", path);
            res.set_toolkit_path(&path);
        }

        if let Ok(windres_path) = std::env::var("WINDRES_x86_64_pc_windows_gnu") {
            println!("cargo:warning=Using windres at: {}", windres_path);
            res.set_windres_path(&windres_path);
        }

        if let Ok(path) = std::env::var("AR_x86_64_pc_windows_gnu") {
            println!("cargo:warning=Using ar at: {}", path);
            res.set_ar_path(&path);
        }

        if let Err(e) = res.compile() {
            eprintln!("Failed to compile Windows resources: {}", e);
            eprintln!("Error kind: {:?}", e.kind());
            eprintln!("Current directory: {:?}", std::env::current_dir());
            eprintln!(
                "Icon path exists: {}",
                std::path::Path::new("../liana-ui/static/logos/liana.ico").exists()
            );
            panic!("Windows resource compilation failed: {}", e);
        }
    }
}

fn main() {
    // Windows resource configuration from master
    #[cfg(windows)]
    winresource::WindowsResource::new()
        .set_icon("../liana-ui/static/logos/liana-app-icon-coincube.ico")
        .compile()
        .unwrap();

    // Fix for missing UltralightCore library link
    // The ul-next-sys build script is missing the UltralightCore link directive
    // but WebCore depends on symbols from UltralightCore (like FastBlur)
    println!("cargo:rustc-link-lib=dylib=UltralightCore");
}

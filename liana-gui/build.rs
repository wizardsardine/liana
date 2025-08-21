fn main() {
    #[cfg(feature = "webview")]
    {
        // Add UltralightCore library linking to fix FastBlur undefined reference
        // This is needed because ul-next-sys doesn't link against UltralightCore
        // but WebCore depends on symbols from it
        println!("cargo:rustc-link-lib=dylib=UltralightCore");
        
        // Rerun if webview feature changes
        println!("cargo:rerun-if-changed=Cargo.toml");
    }
}

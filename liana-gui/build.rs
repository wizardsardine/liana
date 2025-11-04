fn main() {
    // Windows resource configuration from master
    #[cfg(windows)]
    winresource::WindowsResource::new()
        .set_icon("../liana-ui/static/logos/liana-app-icon-coincube.ico")
        .compile()
        .unwrap();
}

fn main() {
    #[cfg(windows)]
    winresource::WindowsResource::new()
        .set_icon("../liana-ui/static/logos/liana-app-icon-coincube.ico")
        .compile()
        .unwrap();
}

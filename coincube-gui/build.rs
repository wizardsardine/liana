fn main() {
    // Windows resource configuration from master
    #[cfg(windows)]
    winresource::WindowsResource::new()
        .set_icon("../coincube-ui/static/logos/coincube-cc.ico")
        .compile()
        .unwrap();
}

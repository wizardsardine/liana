use dotenvy::dotenv;

fn main() {
    if cfg!(debug_assertions) {
        // Load the .env file from the project root
        if dotenv().is_ok() {
            for (key, value) in dotenvy::vars() {
                println!("cargo:rustc-env={}={}", key, value);
            }
        };
    }

    // Windows resource configuration from master
    #[cfg(windows)]
    winresource::WindowsResource::new()
        .set_icon("../coincube-ui/static/logos/coincube-cc.ico")
        .compile()
        .unwrap();
}

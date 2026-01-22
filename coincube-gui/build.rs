use dotenvy::dotenv;

fn main() {
    if cfg!(debug_assertions) {
        // Load the .env file from the project root
        let _ = dotenv().expect("Failed to read .env file");

        // Iterate over all environment variables loaded by dotenvy and pass them to rustc
        for (key, value) in dotenvy::vars() {
            // Check if the variable is defined in the .env file (optional)
            // If you only want variables from the .env file, iterate using dotenvy::dotenv_iter()
            // and filter
            println!("cargo:rustc-env={}={}", key, value);
        }
    }

    // Windows resource configuration from master
    #[cfg(windows)]
    winresource::WindowsResource::new()
        .set_icon("../coincube-ui/static/logos/coincube-cc.ico")
        .compile()
        .unwrap();
}

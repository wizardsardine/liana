use dotenvy::dotenv;

fn main() {
    if cfg!(debug_assertions) {
        // Load the .env file from the project root
        if dotenv().is_ok() {
            let env = dotenvy::dotenv_iter();
            if let Ok(iter) = env {
                for (key, value) in iter.flatten() {
                    println!("cargo:rustc-env={}={}", key, value);
                }
            }
        };
    }

    #[cfg(windows)]
    {
        // increase stack size to 8MB
        println!("cargo:rustc-link-arg-bins=/STACK:8000000");

        // Windows resource configuration from master
        winresource::WindowsResource::new()
            .set_icon("../coincube-ui/static/logos/coincube-cc.ico")
            .compile()
            .unwrap();
    }
}

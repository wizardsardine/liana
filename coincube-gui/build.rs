use dotenvy::dotenv;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Re-run this build script whenever .env changes so rustc-env vars stay current
    println!("cargo:rerun-if-changed=../.env");

    // Load the .env file from the project root for all build profiles
    if dotenv().is_ok() {
        let env = dotenvy::dotenv_iter();
        if let Ok(iter) = env {
            for (key, value) in iter.flatten() {
                println!("cargo:rustc-env={}={}", key, value);
            }
        }
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

    // gRPC proto codegen
    println!("cargo:rerun-if-changed=../grpc/connect.proto");
    tonic_build::configure()
        .build_server(false)
        .build_transport(false) // Avoid generating `connect()` which uses TryInto (edition 2021)
        .compile_protos(&["../grpc/connect.proto"], &["../grpc/"])?;

    // Local-signer wire format (LAN transport for the Keychain phone
    // app). connect.v1 types are mapped via extern_path to the
    // already-generated module in
    // coincube-gui/src/services/connect/grpc/mod.rs, so we don't
    // re-emit them. The generated file is written to a dedicated
    // sub-OUT_DIR so the extern stubs don't clobber the real
    // `connect.v1.rs` written by the first invocation above.
    println!("cargo:rerun-if-changed=../grpc/local_envelope.proto");
    let local_out =
        std::path::PathBuf::from(std::env::var("OUT_DIR")?).join("local_envelope");
    std::fs::create_dir_all(&local_out)?;
    tonic_build::configure()
        .build_server(false)
        .build_transport(false)
        .out_dir(&local_out)
        .extern_path(".connect.v1", "crate::services::connect::grpc::connect_v1")
        .compile_protos(&["../grpc/local_envelope.proto"], &["../grpc/"])?;

    Ok(())
}

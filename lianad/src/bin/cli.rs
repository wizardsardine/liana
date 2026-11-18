#![cfg(not(target_os = "windows"))]

use lianad::config::Config;

use std::{
    env,
    io::{Read, Write},
    path::PathBuf,
    process,
};

use serde_json::Value as Json;

use std::os::unix::net::UnixStream;

// Exits with error
fn show_usage() {
    eprintln!("Usage:");
    eprintln!(" liana-cli [--conf conf_path] [--raw] <command> [<param 1> <param 2> ...]");
    process::exit(1);
}

// Returns (Maybe(special conf file), Raw, Method name, Maybe(List of parameters))
fn parse_args(mut args: Vec<String>) -> (Option<PathBuf>, bool, String, Vec<String>) {
    if args.len() < 2 {
        eprintln!("Not enough arguments.");
        show_usage();
    }

    args.remove(0); // Program name

    let mut args = args.into_iter();
    let mut raw = false;
    let mut conf_file = None;

    loop {
        match args.next().as_deref() {
            Some("--conf") => {
                if args.len() < 2 {
                    eprintln!("Not enough arguments.");
                    show_usage();
                }

                conf_file = Some(PathBuf::from(args.next().expect("Just checked")));
            }
            Some("--raw") => {
                if args.len() < 1 {
                    eprintln!("Not enough arguments.");
                    show_usage();
                }
                raw = true;
            }
            Some(method) => return (conf_file, raw, method.to_owned(), args.collect()),
            None => {
                // Should never happen...
                eprintln!("Not enough arguments.");
                show_usage();
            }
        }
    }
}

// Defaults to String Value when parsing fails, as it fails to parse outpoints otherwise...
fn from_str_hack(token: String) -> Json {
    match serde_json::from_str(&token) {
        Ok(json) => json,
        Err(_) => Json::String(token),
    }
}

fn rpc_request(method: String, params: Vec<String>) -> Json {
    let method = Json::String(method);
    let params = Json::Array(params.into_iter().map(from_str_hack).collect::<Vec<Json>>());
    let mut object = serde_json::Map::<String, Json>::new();
    object.insert("jsonrpc".to_string(), Json::String("2.0".to_string()));
    object.insert(
        "id".to_string(),
        Json::String(format!("liana-cli-{}", process::id())),
    );
    object.insert("method".to_string(), method);
    object.insert("params".to_string(), params);

    Json::Object(object)
}

fn socket_file(conf_file: Option<PathBuf>) -> PathBuf {
    let config = Config::from_file(conf_file).unwrap_or_else(|e| {
        eprintln!("Error getting config: {e}");
        process::exit(1);
    });
    let data_dir = config
        .data_directory()
        .expect("Wallet datadir is not properly defined");
    data_dir.lianad_rpc_socket_path()
}

fn main() {
    let args = env::args().collect();
    let (conf_file, raw, method, params) = parse_args(args);
    let request = rpc_request(method, params);
    let socket_file = socket_file(conf_file);
    let mut raw_response = Vec::new();
    let mut chunk = [0u8; 4096];

    let mut socket = UnixStream::connect(&socket_file).unwrap_or_else(|e| {
        eprintln!("Could not connect to {socket_file:?}: '{e}'");
        process::exit(1);
    });
    socket
        .write_all(&[request.to_string().as_bytes(), b"\n"].concat())
        .unwrap_or_else(|e| {
            eprintln!("Writing to {:?}: '{}'", &socket_file, e);
            process::exit(1);
        });

    loop {
        let n = socket.read(&mut chunk).unwrap_or_else(|e| {
            eprintln!("Reading from {:?}: '{}'", &socket_file, e);
            process::exit(1);
        });
        if n == 0 {
            eprintln!(
                "Reading from {:?}: '{}'",
                &socket_file,
                std::io::Error::from(std::io::ErrorKind::UnexpectedEof)
            );
            process::exit(1);
        }
        raw_response.extend_from_slice(&chunk[..n]);

        match serde_json::from_slice::<Json>(&raw_response) {
            Ok(response) => {
                if response.get("id") == request.get("id") {
                    if raw {
                        print!("{response}");
                    } else if let Some(r) = response.get("result") {
                        println!("{:#}", serde_json::json!({ "result": r }));
                    } else if let Some(e) = response.get("error") {
                        println!("{:#}", serde_json::json!({ "error": e }));
                    } else {
                        eprintln!(
                            "Reading from {:?}: 'Response contains neither result nor error'",
                            &socket_file
                        );
                        process::exit(1);
                    }
                    return;
                }
            }
            Err(e) if e.is_eof() => continue,
            Err(e) => {
                eprintln!("Reading from {:?}: '{}'", &socket_file, e);
                process::exit(1);
            }
        }
    }
}

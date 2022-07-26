use minisafe::config::{config_folder_path, Config};

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
    eprintln!(" revault-cli [--conf conf_path] [--raw] <command> [<param 1> <param 2> ...]");
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
        Json::String(format!("revault-cli-{}", process::id())),
    );
    object.insert("method".to_string(), method);
    object.insert("params".to_string(), params);

    Json::Object(object)
}

fn socket_file(conf_file: Option<PathBuf>) -> PathBuf {
    let config = Config::from_file(conf_file).unwrap_or_else(|e| {
        eprintln!("Error getting config: {}", e);
        process::exit(1);
    });
    let data_dir = config
        .data_dir
        .unwrap_or_else(|| config_folder_path().unwrap());
    let data_dir = data_dir.to_str().expect("Datadir is valid unicode");

    [
        data_dir,
        config.bitcoind_config.network.to_string().as_str(),
        "revaultd_rpc",
    ]
    .iter()
    .collect()
}

fn trimmed(mut vec: Vec<u8>, bytes_read: usize) -> Vec<u8> {
    vec.truncate(bytes_read);

    // Until there is some whatever-newline character, pop.
    while let Some(byte) = vec.last() {
        // Of course, we assume utf-8
        if !(&0x0a..=&0x0d).contains(&byte) {
            break;
        }
        vec.pop();
    }

    vec
}

fn main() {
    let args = env::args().collect();
    let (conf_file, raw, method, params) = parse_args(args);
    let request = rpc_request(method, params);
    let socket_file = socket_file(conf_file);
    let mut raw_response = vec![0; 256];

    let mut socket = UnixStream::connect(&socket_file).unwrap_or_else(|e| {
        eprintln!("Could not connect to {:?}: '{}'", socket_file, e);
        process::exit(1);
    });
    socket
        .write_all(request.to_string().as_bytes())
        .unwrap_or_else(|e| {
            eprintln!("Writing to {:?}: '{}'", &socket_file, e);
            process::exit(1);
        });

    let mut total_read = 0;
    loop {
        let n = socket
            .read(&mut raw_response[total_read..])
            .unwrap_or_else(|e| {
                eprintln!("Reading from {:?}: '{}'", &socket_file, e);
                process::exit(1);
            });
        total_read += n;
        if total_read == raw_response.len() {
            raw_response.resize(2 * total_read, 0);
            continue;
        }

        // FIXME: do actual incremental parsing instead of this hack!!
        raw_response = trimmed(raw_response, total_read);
        match serde_json::from_slice::<Json>(&raw_response) {
            Ok(response) => {
                if response.get("id") == request.get("id") {
                    if raw {
                        print!("{}", response);
                    } else if let Some(r) = response.get("result") {
                        println!("{:#}", serde_json::json!({ "result": r }));
                    } else if let Some(e) = response.get("error") {
                        println!("{:#}", serde_json::json!({ "error": e }));
                    } else {
                        log::warn!(
                            "revaultd response doesn't contain result or error: '{}'",
                            response
                        );
                        println!("{:#}", response);
                    }
                    return;
                }
            }
            Err(_) => continue,
        }
    }
}

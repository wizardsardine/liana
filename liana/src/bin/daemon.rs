use std::{
    env,
    io::{self, Write},
    path::PathBuf,
    process, thread, time,
};

use liana::{config::Config, DaemonHandle, VERSION};

fn print_help_exit(code: i32) {
    eprintln!("lianad version {}", VERSION);
    eprintln!("A TOML configuration file is required to run lianad. By default lianad looks for a 'config.toml' file in its data directory. A different one may be provided like so: '--conf <config file path>'.");
    eprintln!("A documented sample is available at 'contrib/lianad_config_example.toml' in the source tree (https://github.com/wizardsardine/liana/blob/v1.0/contrib/lianad_config_example.toml).");
    eprintln!("The default data directory path is a 'liana/' folder in the XDG standard configuration directory for all OSes but Linux ones, where it's '~/.liana/'.");
    process::exit(code);
}

fn print_version() {
    eprintln!("{}", VERSION);
    process::exit(0);
}

fn parse_args(args: Vec<String>) -> Option<PathBuf> {
    if args.len() == 1 {
        return None;
    }

    if args[1] == "--help" || args[1] == "-h" {
        print_help_exit(0)
    } else if args[1] == "--version" || args[1] == "-v" {
        print_version()
    } else if args[1] != "--conf" {
        eprintln!("Only a single command line argument is supported: --conf. All other configuration parameters must be specified in the configuration file.");
        print_help_exit(1);
    }

    if args.len() != 3 {
        print_help_exit(1);
    }

    Some(PathBuf::from(args[2].to_owned()))
}

fn setup_logger(log_level: log::LevelFilter) -> Result<(), fern::InitError> {
    let dispatcher = fern::Dispatch::new()
        .format(|out, message, record| {
            out.finish(format_args!(
                "[{}][{}][{}][thread {}] {}",
                time::SystemTime::now()
                    .duration_since(time::UNIX_EPOCH)
                    .unwrap_or_else(|e| {
                        println!("Can't get time since epoch: '{}'. Using a dummy value.", e);
                        time::Duration::from_secs(0)
                    })
                    .as_secs(),
                record.target(),
                record.level(),
                thread::current().name().unwrap_or("unnamed"),
                message
            ))
        })
        .level(log_level);

    dispatcher.chain(std::io::stdout()).apply()?;

    Ok(())
}

fn main() {
    let args = env::args().collect();
    let conf_file = parse_args(args);

    let config = Config::from_file(conf_file).unwrap_or_else(|e| {
        eprintln!("Error parsing config: {}", e);
        print_help_exit(1);
        unreachable!();
    });
    setup_logger(config.log_level).unwrap_or_else(|e| {
        eprintln!("Error setting up logger: {}", e);
        process::exit(1);
    });

    let handle = DaemonHandle::start_default(config, cfg!(unix)).unwrap_or_else(|e| {
        log::error!("Error starting Liana daemon: {}", e);
        process::exit(1);
    });
    while handle.is_alive() {
        thread::sleep(time::Duration::from_millis(500));
    }
    if let Err(e) = handle.stop() {
        log::error!("Error stopping Liana daemon: {}", e);
    }

    // We are always logging to stdout, should it be then piped to the log file (if self) or
    // not. So just make sure that all messages were actually written.
    io::stdout().flush().expect("Flushing stdout");
}

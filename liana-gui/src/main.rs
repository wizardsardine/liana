#![windows_subsystem = "windows"]

use std::{error::Error, io::Write, path::PathBuf, process, str::FromStr};

#[cfg(target_os = "linux")]
use iced::window::settings::PlatformSpecific;
use iced::{Settings, Size};
use tracing::error;
use tracing_subscriber::filter::LevelFilter;
extern crate serde;
extern crate serde_json;

use liana::miniscript::bitcoin;
use liana_ui::{component::text, font, image, theme};

use liana_gui::{
    app::settings::global::{GlobalSettings, WindowConfig},
    dir::LianaDirectory,
    gui::{Config, GUI},
    node::bitcoind::delete_all_bitcoind_locks_for_process,
    VERSION,
};

#[derive(Debug, PartialEq)]
enum Arg {
    DatadirPath(LianaDirectory),
    Network(bitcoin::Network),
}

fn parse_args(args: Vec<String>) -> Result<Vec<Arg>, Box<dyn Error>> {
    let mut res = Vec::new();

    if args.len() > 1 && (args[1] == "--version" || args[1] == "-v") {
        eprintln!("{}", VERSION);
        process::exit(1);
    }

    if args.len() > 1 && (args[1] == "--help" || args[1] == "-h") {
        eprintln!(
            r#"
Usage: liana-gui [OPTIONS]

Options:
    --datadir <PATH>    Path of liana datadir
    -v, --version       Display liana-gui version
    -h, --help          Print help
    --bitcoin           Use bitcoin network
    --testnet           Use testnet network
    --signet            Use signet network
    --regtest           Use regtest network
        "#
        );
        process::exit(1);
    }

    for (i, arg) in args.iter().enumerate() {
        if arg == "--datadir" {
            if let Some(a) = args.get(i + 1) {
                res.push(Arg::DatadirPath(LianaDirectory::new(PathBuf::from(a))));
            } else {
                return Err("missing arg to --datadir".into());
            }
        } else if arg.contains("--") {
            let network = bitcoin::Network::from_str(args[i].trim_start_matches("--"))?;
            res.push(Arg::Network(network));
        }
    }

    Ok(res)
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = parse_args(std::env::args().collect())?;
    let config = match args.as_slice() {
        [] => {
            let datadir_path = LianaDirectory::new_default().unwrap();
            Config::new(datadir_path, None)
        }
        [Arg::Network(network)] => {
            let datadir_path = LianaDirectory::new_default().unwrap();
            Config::new(datadir_path, Some(*network))
        }
        [Arg::DatadirPath(datadir_path)] => Config::new(datadir_path.clone(), None),
        [Arg::DatadirPath(datadir_path), Arg::Network(network)]
        | [Arg::Network(network), Arg::DatadirPath(datadir_path)] => {
            Config::new(datadir_path.clone(), Some(*network))
        }
        _ => {
            return Err("Unknown args combination".into());
        }
    };

    let log_level = if let Ok(l) = std::env::var("LOG_LEVEL") {
        Some(LevelFilter::from_str(&l)?)
    } else {
        None
    };

    setup_panic_hook(&config.liana_directory);

    let settings = Settings {
        id: Some("Liana".to_string()),
        antialiasing: false,

        default_text_size: text::P1_SIZE.into(),
        default_font: liana_ui::font::REGULAR,
        fonts: font::load(),
    };

    let global_config_path = GlobalSettings::path(&config.liana_directory);
    let initial_size = if let Some(WindowConfig { width, height }) =
        GlobalSettings::load_window_config(&global_config_path)
    {
        Size { width, height }
    } else {
        iced::window::Settings::default().size
    };

    #[allow(unused_mut)]
    let mut window_settings = iced::window::Settings {
        size: initial_size,
        icon: Some(image::liana_app_icon()),
        position: iced::window::Position::Default,
        min_size: Some(Size {
            width: 1000.0,
            height: 650.0,
        }),
        exit_on_close_request: false,
        ..Default::default()
    };

    #[cfg(target_os = "linux")]
    {
        window_settings.platform_specific = PlatformSpecific {
            application_id: "Liana".to_string(),
            ..Default::default()
        };
    }

    if let Err(e) = iced::application(GUI::title, GUI::update, GUI::view)
        .theme(|_| theme::Theme::default())
        .scale_factor(GUI::scale_factor)
        .subscription(GUI::subscription)
        .settings(settings)
        .window(window_settings)
        .run_with(move || GUI::new((config, log_level)))
    {
        log::error!("{}", e);
        Err(format!("Failed to launch UI: {}", e).into())
    } else {
        Ok(())
    }
}

// A panic in any thread should stop the main thread, and print the panic.
fn setup_panic_hook(liana_directory: &LianaDirectory) {
    let bitcoind_dir = liana_directory.bitcoind_directory();
    std::panic::set_hook(Box::new(move |panic_info| {
        error!("Panic occurred");
        if let Err(e) = delete_all_bitcoind_locks_for_process(bitcoind_dir.clone()) {
            error!("Failed to delete internal bitcoind locks: {}", e);
        }
        let file = panic_info
            .location()
            .map(|l| l.file())
            .unwrap_or_else(|| "'unknown'");
        let line = panic_info
            .location()
            .map(|l| l.line().to_string())
            .unwrap_or_else(|| "'unknown'".to_string());

        let bt = backtrace::Backtrace::new();
        let info = panic_info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| panic_info.payload().downcast_ref::<String>().cloned());
        error!(
            "panic occurred at line {} of file {}: {:?}\n{:?}",
            line, file, info, bt
        );

        std::io::stdout().flush().expect("Flushing stdout");
        std::process::exit(1);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use liana_gui::dir::LianaDirectory;

    #[test]
    fn test_parse_args() {
        assert!(parse_args(vec!["--meth".into()]).is_err());
        assert!(parse_args(vec!["--datadir".into()]).is_err());
        assert_eq!(
            Some(vec![Arg::Network(bitcoin::Network::Regtest)]),
            parse_args(vec!["--regtest".into()]).ok()
        );
        assert_eq!(
            Some(vec![
                Arg::DatadirPath(LianaDirectory::new(PathBuf::from("hello"))),
                Arg::Network(bitcoin::Network::Testnet)
            ]),
            parse_args(
                "--datadir hello --testnet"
                    .split(' ')
                    .map(|a| a.to_string())
                    .collect()
            )
            .ok()
        );
        assert_eq!(
            Some(vec![
                Arg::Network(bitcoin::Network::Testnet),
                Arg::DatadirPath(LianaDirectory::new(PathBuf::from("hello"))),
            ]),
            parse_args(
                "--testnet --datadir hello"
                    .split(' ')
                    .map(|a| a.to_string())
                    .collect()
            )
            .ok()
        );
    }
}

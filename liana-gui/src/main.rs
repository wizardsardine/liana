#![windows_subsystem = "windows"]

use std::{error::Error, fmt::Display, io::Write, path::PathBuf, process, str::FromStr};

#[cfg(target_os = "linux")]
use iced::window::settings::PlatformSpecific;
use iced::{Settings, Size};
use tracing::error;
use tracing_subscriber::filter::LevelFilter;
extern crate serde;
extern crate serde_json;

use liana::miniscript::bitcoin::Network;
use liana_ui::{component::text, font, image, theme};

use liana_gui::{
    app::settings::global::{GlobalSettings, WindowConfig},
    dir::LianaDirectory,
    gui::{Config, LianaGUI},
    node::bitcoind::delete_all_bitcoind_locks_for_process,
    VERSION,
};

#[derive(Debug, Clone, PartialEq)]
enum Arg {
    DatadirPath(LianaDirectory),
    Network(Network),
}

fn parse_args(
    args: Vec<String>,
    version: impl Display,
    available_networks: &[Network],
    default_network: Option<Network>,
) -> Result<Vec<Arg>, Box<dyn Error>> {
    let mut res = Vec::new();

    let app_name = std::path::Path::new(&args[0])
        .file_name()
        .and_then(|s| s.to_str())
        // This should never happen
        .unwrap_or("liana");

    if args.len() > 1 && (args[1] == "--version" || args[1] == "-v") {
        eprintln!("{}", version);
        process::exit(0);
    }

    if args.len() > 1 && (args[1] == "--help" || args[1] == "-h") {
        let network_options: String = available_networks
            .iter()
            .map(|n| {
                let name = n.to_string().to_lowercase();
                let default_marker = if Some(*n) == default_network {
                    " (default)"
                } else {
                    ""
                };
                format!("    --{:<15} Use {} network{}", name, name, default_marker)
            })
            .collect::<Vec<_>>()
            .join("\n");

        eprintln!(
            r#"
Usage: {app_name} [OPTIONS]

Options:
    --datadir <PATH>    Path of liana datadir
    -v, --version       Display {app_name} version
    -h, --help          Print help
{network_options}
        "#
        );
        process::exit(0);
    }

    for (i, arg) in args.iter().enumerate() {
        if arg == "--datadir" {
            if let Some(a) = args.get(i + 1) {
                res.push(Arg::DatadirPath(LianaDirectory::new(PathBuf::from(a))));
            } else {
                return Err("missing arg to --datadir".into());
            }
        } else if arg.starts_with("--") && arg != "--datadir" {
            let network = Network::from_str(arg.trim_start_matches("--"))?;
            if !available_networks.contains(&network) {
                return Err(format!("network {} is not available", network).into());
            }
            res.push(Arg::Network(network));
        }
    }

    Ok(res)
}

fn main() -> Result<(), Box<dyn Error>> {
    use Network::{Bitcoin, Regtest, Signet, Testnet};
    let args = parse_args(
        std::env::args().collect(),
        VERSION,
        &[Bitcoin, Testnet, Signet, Regtest],
        None,
    )?;
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

    if let Err(e) = iced::application(LianaGUI::title, LianaGUI::update, LianaGUI::view)
        .theme(|_| theme::Theme::default())
        .scale_factor(LianaGUI::scale_factor)
        .subscription(LianaGUI::subscription)
        .settings(settings)
        .window(window_settings)
        .run_with(move || LianaGUI::new((config, log_level)))
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
    use Network::{Bitcoin, Regtest, Signet, Testnet};

    const ALL_NETWORKS: &[Network] = &[Bitcoin, Testnet, Signet, Regtest];

    #[test]
    fn test_parse_args() {
        assert!(parse_args(
            vec!["app".into(), "--meth".into()],
            VERSION,
            ALL_NETWORKS,
            None
        )
        .is_err());
        assert!(parse_args(
            vec!["app".into(), "--datadir".into()],
            VERSION,
            ALL_NETWORKS,
            None
        )
        .is_err());
        assert_eq!(
            Some(vec![Arg::Network(Regtest)]),
            parse_args(
                vec!["app".into(), "--regtest".into()],
                VERSION,
                ALL_NETWORKS,
                None
            )
            .ok()
        );
        assert_eq!(
            Some(vec![
                Arg::DatadirPath(LianaDirectory::new(PathBuf::from("hello"))),
                Arg::Network(Testnet)
            ]),
            parse_args(
                "app --datadir hello --testnet"
                    .split(' ')
                    .map(|a| a.to_string())
                    .collect(),
                VERSION,
                ALL_NETWORKS,
                None
            )
            .ok()
        );
        assert_eq!(
            Some(vec![
                Arg::Network(Testnet),
                Arg::DatadirPath(LianaDirectory::new(PathBuf::from("hello"))),
            ]),
            parse_args(
                "app --testnet --datadir hello"
                    .split(' ')
                    .map(|a| a.to_string())
                    .collect(),
                VERSION,
                ALL_NETWORKS,
                None
            )
            .ok()
        );
    }
}

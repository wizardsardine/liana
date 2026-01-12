#![windows_subsystem = "windows"]

use std::{error::Error, io::Write, str::FromStr};

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
    utils::args::{parse_args, Arg},
    VERSION,
};

/// Convert parsed command-line arguments to a Config.
///
/// # Arguments
/// - `args`: Parsed command-line arguments
/// - `default_network`: Network to use when none is specified in args
fn args_to_config(
    args: &[Arg],
    default_network: Option<Network>,
) -> Result<Config, Box<dyn Error>> {
    match args {
        [] => {
            let datadir_path = LianaDirectory::new_default().unwrap();
            Ok(Config::new(datadir_path, default_network))
        }
        [Arg::Network(network)] => {
            let datadir_path = LianaDirectory::new_default().unwrap();
            Ok(Config::new(datadir_path, Some(*network)))
        }
        [Arg::DatadirPath(datadir_path)] => Ok(Config::new(datadir_path.clone(), default_network)),
        [Arg::DatadirPath(datadir_path), Arg::Network(network)]
        | [Arg::Network(network), Arg::DatadirPath(datadir_path)] => {
            Ok(Config::new(datadir_path.clone(), Some(*network)))
        }
        _ => Err("Unknown args combination".into()),
    }
}

/// Parse LOG_LEVEL environment variable.
fn parse_log_level() -> Result<Option<LevelFilter>, Box<dyn Error>> {
    if let Ok(l) = std::env::var("LOG_LEVEL") {
        Ok(Some(LevelFilter::from_str(&l)?))
    } else {
        Ok(None)
    }
}

/// Create iced application Settings.
fn create_app_settings(app_id: &str) -> Settings {
    Settings {
        id: Some(app_id.to_string()),
        antialiasing: false,
        default_text_size: text::P1_SIZE.into(),
        default_font: liana_ui::font::REGULAR,
        fonts: font::load(),
    }
}

/// Load initial window size from global settings, or use default.
fn load_initial_size(liana_directory: &LianaDirectory, default_size: Option<Size>) -> Size {
    let global_config_path = GlobalSettings::path(liana_directory);
    if let Some(WindowConfig { width, height }) =
        GlobalSettings::load_window_config(&global_config_path)
    {
        Size { width, height }
    } else {
        default_size.unwrap_or(iced::window::Settings::default().size)
    }
}

/// Create iced window Settings.
#[allow(unused_mut)]
fn create_window_settings(app_id: &str, initial_size: Size) -> iced::window::Settings {
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
            application_id: app_id.to_string(),
            ..Default::default()
        };
    }

    window_settings
}

fn main() -> Result<(), Box<dyn Error>> {
    use Network::{Bitcoin, Regtest, Signet, Testnet4};

    let args = parse_args(
        std::env::args().collect(),
        VERSION,
        &[Bitcoin, Testnet4, Signet, Regtest],
        None,
    )?;

    let config = args_to_config(&args, None)?;
    let log_level = parse_log_level()?;

    setup_panic_hook(&config.liana_directory);

    let settings = create_app_settings("Liana");
    let initial_size = load_initial_size(&config.liana_directory, None);
    let window_settings = create_window_settings("Liana", initial_size);

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

//! Application setup utilities shared between liana-gui and liana-business.

use std::{error::Error, str::FromStr};

#[cfg(target_os = "linux")]
use iced::window::settings::PlatformSpecific;
use iced::{Settings, Size};
use tracing_subscriber::filter::LevelFilter;

use liana::miniscript::bitcoin::Network;
use liana_ui::{component::text, font, image};

use crate::{
    app::settings::global::{GlobalSettings, WindowConfig},
    dir::LianaDirectory,
    gui::Config,
    utils::args::Arg,
};

/// Convert parsed command-line arguments to a Config.
///
/// # Arguments
/// - `args`: Parsed command-line arguments
/// - `default_network`: Network to use when none is specified in args
pub fn args_to_config(
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
pub fn parse_log_level() -> Result<Option<LevelFilter>, Box<dyn Error>> {
    if let Ok(l) = std::env::var("LOG_LEVEL") {
        Ok(Some(LevelFilter::from_str(&l)?))
    } else {
        Ok(None)
    }
}

/// Create iced application Settings.
pub fn create_app_settings(app_id: &str) -> Settings {
    Settings {
        id: Some(app_id.to_string()),
        antialiasing: false,
        default_text_size: text::P1_SIZE.into(),
        default_font: font::REGULAR,
        fonts: font::load(),
    }
}

/// Load initial window size from global settings, or use default.
pub fn load_initial_size(liana_directory: &LianaDirectory, default_size: Option<Size>) -> Size {
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
pub fn create_window_settings(app_id: &str, initial_size: Size) -> iced::window::Settings {
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

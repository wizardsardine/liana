//! Liana Business - Policy Template Builder
//!
//! This application provides the full GUI experience for liana-business,
//! using the standard `GUI<I, S, M>` framework from liana-gui with
//! BusinessInstaller and BusinessSettings implementations.

#![windows_subsystem = "windows"]

use std::{error::Error, io::Write, str::FromStr};

#[cfg(target_os = "linux")]
use iced::window::settings::PlatformSpecific;
use iced::{Settings, Size};
use tracing::error;
use tracing_subscriber::filter::LevelFilter;

use liana::miniscript::bitcoin;
use liana_ui::{component::text, font, image, theme};

use business_installer::{BusinessInstaller, Message};
use business_settings::BusinessSettings;
use liana_gui::{
    app::settings::global::{GlobalSettings, WindowConfig},
    dir::LianaDirectory,
    gui::{Config, GUI},
    utils::args::{parse_args, Arg},
    VERSION,
};

/// Type alias for liana-business GUI.
///
/// This combines the BusinessInstaller (policy builder flow) with BusinessSettings
/// (wallet configuration without bitcoind) in the standard GUI framework.
pub type LianaBusiness = GUI<BusinessInstaller, BusinessSettings, Message>;

fn main() -> Result<(), Box<dyn Error>> {
    use bitcoin::Network::{Bitcoin, Signet};

    // FIXME: change before release
    let default_network = Signet;

    let args = parse_args(
        std::env::args().collect(),
        VERSION,
        &[Bitcoin, Signet],
        Some(default_network),
    )?;

    let config = match args.as_slice() {
        [] => {
            let datadir_path = LianaDirectory::new_default().unwrap();
            Config::new(datadir_path, Some(default_network))
        }
        [Arg::Network(network)] => {
            let datadir_path = LianaDirectory::new_default().unwrap();
            Config::new(datadir_path, Some(*network))
        }
        [Arg::DatadirPath(datadir_path)] => {
            Config::new(datadir_path.clone(), Some(default_network))
        }
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

    setup_panic_hook();

    let settings = Settings {
        id: Some("LianaBusiness".to_string()),
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
        // Default size for liana-business (larger than liana-gui default)
        Size::new(1200.0, 800.0)
    };

    #[allow(unused_mut)]
    let mut window_settings = iced::window::Settings {
        size: initial_size,
        icon: Some(image::liana_app_icon()), // TODO: Use custom liana-business icon
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
            application_id: "LianaBusiness".to_string(),
            ..Default::default()
        };
    }

    if let Err(e) = iced::application(
        LianaBusiness::title,
        LianaBusiness::update,
        LianaBusiness::view,
    )
    .theme(|_| theme::Theme::default())
    .scale_factor(LianaBusiness::scale_factor)
    .subscription(LianaBusiness::subscription)
    .settings(settings)
    .window(window_settings)
    .run_with(move || LianaBusiness::new((config, log_level)))
    {
        log::error!("{}", e);
        Err(format!("Failed to launch UI: {}", e).into())
    } else {
        Ok(())
    }
}

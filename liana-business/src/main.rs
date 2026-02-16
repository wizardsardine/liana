//! Liana Business - Policy Template Builder
//!
//! This application provides the full GUI experience for liana-business,
//! using the standard `GUI<I, S, M>` framework from liana-gui with
//! BusinessInstaller and BusinessSettings implementations.

#![windows_subsystem = "windows"]

use std::error::Error;

use iced::Size;

use liana::miniscript::bitcoin;
use liana_ui::{image, theme};

use business_installer::{BusinessInstaller, Message};
use business_settings::BusinessSettings;
use liana_gui::{
    args::{args_to_config, parse_args},
    gui::GUI,
    logger::parse_log_level,
    window::{create_app_settings, create_window_settings, load_initial_size},
    VERSION,
};

/// Type alias for liana-business GUI.
///
/// This combines the BusinessInstaller (policy builder flow) with BusinessSettings
/// (wallet configuration without bitcoind) in the standard GUI framework.
pub type LianaBusiness = GUI<BusinessInstaller, BusinessSettings, Message>;

fn main() -> Result<(), Box<dyn Error>> {
    use bitcoin::Network::{Bitcoin, Signet};

    let default_network = Bitcoin;

    // Check if Signet is enabled via environment variable
    let signet_enabled = std::env::var("LIANA_BUSINESS_SIGNET")
        .map(|v| v == "1")
        .unwrap_or(false);

    let available_networks: Vec<bitcoin::Network> = if signet_enabled {
        vec![Bitcoin, Signet]
    } else {
        vec![Bitcoin]
    };

    let args = parse_args(
        std::env::args().collect(),
        VERSION,
        &available_networks,
        Some(default_network),
    )?;

    let config = args_to_config(&args, Some(default_network), "Liana Business".into())?;
    let log_level = parse_log_level()?;

    let settings = create_app_settings("LianaBusiness");
    let initial_size = load_initial_size(&config.liana_directory, Some(Size::new(1200.0, 800.0)));
    let mut window_settings = create_window_settings("LianaBusiness", initial_size);
    // Use business-specific app icon (blue instead of green)
    window_settings.icon = Some(image::liana_business_app_icon());

    if let Err(e) = iced::application(
        LianaBusiness::title,
        LianaBusiness::update,
        LianaBusiness::view,
    )
    .theme(|_| theme::Theme::business())
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

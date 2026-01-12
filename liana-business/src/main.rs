//! Liana Business - Policy Template Builder
//!
//! This application provides the full GUI experience for liana-business,
//! using the standard `GUI<I, S, M>` framework from liana-gui with
//! BusinessInstaller and BusinessSettings implementations.

#![windows_subsystem = "windows"]

use std::{error::Error, io::Write};

use iced::Size;
use tracing::error;

use liana::miniscript::bitcoin;
use liana_ui::theme;

use business_installer::{BusinessInstaller, Message};
use business_settings::BusinessSettings;
use liana_gui::{
    gui::GUI,
    utils::{
        app::{args_to_config, create_app_settings, create_window_settings, load_initial_size, parse_log_level},
        args::parse_args,
    },
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

    let config = args_to_config(&args, Some(default_network))?;
    let log_level = parse_log_level()?;

    setup_panic_hook();

    let settings = create_app_settings("LianaBusiness");
    let initial_size = load_initial_size(&config.liana_directory, Some(Size::new(1200.0, 800.0)));
    let window_settings = create_window_settings("LianaBusiness", initial_size);

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

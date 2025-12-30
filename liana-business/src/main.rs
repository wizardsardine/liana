//! Liana Business - Policy Template Builder
//!
//! This is a standalone application that wraps the `BusinessInstaller`
//! component from the `business-installer` crate, which implements the
//! `Installer` trait from liana-gui.

use business_installer::{BusinessInstaller, Message};
use iced::{Pixels, Settings, Subscription, Task};
use liana::miniscript::bitcoin::Network;
use liana_gui::{
    dir::LianaDirectory,
    installer::{Installer, NextState, UserFlow},
};
use liana_ui::theme::Theme;
use tracing_subscriber::{filter::Targets, prelude::*};

fn main() -> iced::Result {
    // Initialize tracing: async_hwi at TRACE, noisy crates silenced, rest at INFO
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .pretty()
                .with_file(false)
                .with_filter(
                    Targets::new()
                        .with_target("async_hwi", tracing::Level::INFO)
                        .with_target("business_installer", tracing::Level::DEBUG)
                        // Silence noisy GPU/window crates
                        .with_target("wgpu_core", tracing::Level::ERROR)
                        .with_target("wgpu_hal", tracing::Level::ERROR)
                        .with_target("iced_wgpu", tracing::Level::ERROR)
                        .with_target("iced_winit", tracing::Level::ERROR)
                        .with_target("winit", tracing::Level::ERROR)
                        .with_target("naga", tracing::Level::ERROR)
                        .with_default(tracing::Level::INFO),
                ),
        )
        .init();
    let settings = Settings {
        default_font: liana_ui::font::REGULAR,
        default_text_size: Pixels(16.0),
        fonts: liana_ui::font::load(),
        ..Default::default()
    };

    let window_settings = iced::window::Settings {
        size: iced::Size::new(1200.0, 800.0),
        ..Default::default()
    };

    iced::application(
        PolicyBuilder::title,
        PolicyBuilder::update,
        PolicyBuilder::view,
    )
    .theme(|_| Theme::default())
    .settings(settings)
    .window(window_settings)
    .subscription(PolicyBuilder::subscription)
    // HACK: poll_next() will hang forever with tokio executor
    .executor::<futures::executor::ThreadPool>()
    .run_with(PolicyBuilder::new)
}

/// PolicyBuilPolicyBuilder::newtion wrapper around BusinessInstaller.
///
/// It implements the Iced application interface and delegates all operations
/// to the inner BusinessInstaller through the Installer trait.
pub struct PolicyBuilder {
    installer: Box<BusinessInstaller>,
}

impl PolicyBuilder {
    pub fn new() -> (Self, Task<Message>) {
        // Create default LianaDirectory
        let datadir =
            LianaDirectory::new_default().expect("Failed to create default data directory");

        // Use Signet network for business installer
        let network = Network::Bitcoin;

        // Create BusinessInstaller via the Installer trait
        let (installer, task) = BusinessInstaller::new(
            datadir,
            network,
            None,                   // No remote backend for now
            UserFlow::CreateWallet, // Default user flow
        );

        let builder = Self { installer };

        (builder, task)
    }

    pub fn title(&self) -> String {
        "Liana Business template builder".to_string()
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        // Process the message first
        let task = self.installer.update(message.clone());

        // Check if we should exit after processing the message
        if let Some(NextState::LoginLianaLite { .. }) = self.installer.exit_maybe(&message) {
            return iced::exit();
        }

        task
    }

    pub fn view(&self) -> liana_ui::widget::Element<Message> {
        self.installer.view()
    }

    pub fn subscription(&self) -> Subscription<Message> {
        self.installer.subscription()
    }
}

impl Default for PolicyBuilder {
    fn default() -> Self {
        Self::new().0
    }
}

// NOTE: No Drop impl needed here - BusinessInstaller already has its own Drop
// implementation that handles cleanup (stop_hw, close_backend)

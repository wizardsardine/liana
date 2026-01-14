#[cfg(target_os = "linux")]
use iced::window::settings::PlatformSpecific;
use iced::{Settings, Size};

use crate::{
    app::settings::global::{GlobalSettings, WindowConfig},
    dir::LianaDirectory,
};
use liana_ui::{component::text, font, image};

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

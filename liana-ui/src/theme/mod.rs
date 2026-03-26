pub mod badge;
pub mod banner;
pub mod button;
pub mod card;
pub mod checkbox;
pub mod container;
pub mod context_menu;
pub mod notification;
pub mod overlay;
pub mod palette;
pub mod pane_grid;
pub mod pick_list;
pub mod pill;
pub mod progress_bar;
pub mod qr_code;
pub mod radio;
pub mod rule;
pub mod scrollable;
pub mod slider;
pub mod svg;
pub mod text;
pub mod text_input;
pub mod toggler;

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Theme {
    pub colors: palette::Palette,
    pub button_border_width: f32,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            colors: palette::Palette::liana(),
            button_border_width: 1.0,
        }
    }
}

impl Theme {
    /// Creates the Liana Business theme (light mode with cyan-blue accent)
    pub fn business() -> Self {
        Self {
            colors: palette::Palette::business(),
            button_border_width: 3.0,
        }
    }
}

impl iced::application::DefaultStyle for Theme {
    fn default_style(&self) -> iced::application::Appearance {
        iced::application::Appearance {
            background_color: self.colors.general.background,
            text_color: self.colors.text.primary,
        }
    }
}

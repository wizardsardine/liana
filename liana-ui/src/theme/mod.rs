pub mod badge;
pub mod banner;
pub mod button;
pub mod card;
pub mod checkbox;
pub mod container;
pub mod notification;
pub mod overlay;
pub mod palette;
pub mod pick_list;
pub mod pill;
pub mod progress_bar;
pub mod qr_code;
pub mod radio;
pub mod scrollable;
pub mod slider;
pub mod svg;
pub mod text;
pub mod text_input;

#[derive(Debug, Copy, Clone, PartialEq, Default)]
pub struct Theme {
    pub colors: palette::Palette,
}

impl iced::application::DefaultStyle for Theme {
    fn default_style(&self) -> iced::application::Appearance {
        iced::application::Appearance {
            background_color: self.colors.general.background,
            text_color: self.colors.text.primary,
        }
    }
}

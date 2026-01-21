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

#[derive(Debug, Copy, Clone, PartialEq, Default)]
pub struct Theme {
    pub colors: palette::Palette,
}

impl iced::theme::Base for Theme {
    fn default(_preference: iced::theme::Mode) -> Self {
        <Self as std::default::Default>::default()
    }

    fn mode(&self) -> iced::theme::Mode {
        iced::theme::Mode::Dark
    }

    fn base(&self) -> iced::theme::Style {
        iced::theme::Style {
            background_color: self.colors.general.background,
            text_color: self.colors.text.primary,
        }
    }

    fn palette(&self) -> Option<iced::theme::Palette> {
        None
    }

    fn name(&self) -> &str {
        "CoincubeTheme"
    }
}

impl iced_aw::style::number_input::Catalog for Theme {
    type Class<'a> = ();

    fn default<'a>() -> Self::Class<'a> {}

    fn style(
        &self,
        _class: &Self::Class<'_>,
        status: iced_aw::card::Status,
    ) -> iced_aw::number_input::Style {
        let (background, icon) = match status {
            iced_aw::card::Status::Active => (
                self.colors.text_inputs.primary.active.background,
                self.colors.text_inputs.primary.active.icon,
            ),
            iced_aw::card::Status::Disabled => (
                self.colors.text_inputs.primary.disabled.background,
                self.colors.text_inputs.primary.disabled.icon,
            ),
            iced_aw::card::Status::Selected => (
                self.colors.text_inputs.primary.active.selection,
                self.colors.text_inputs.primary.disabled.icon,
            ),
            iced_aw::card::Status::Hovered => (
                self.colors.buttons.primary.hovered.background,
                self.colors
                    .buttons
                    .primary
                    .hovered
                    .border
                    .unwrap_or(crate::color::ORANGE),
            ),
            iced_aw::card::Status::Pressed => (
                self.colors.buttons.primary.hovered.background,
                self.colors
                    .buttons
                    .primary
                    .hovered
                    .border
                    .unwrap_or(crate::color::ORANGE),
            ),
            iced_aw::card::Status::Focused => (
                self.colors.text_inputs.primary.active.background,
                self.colors.text_inputs.primary.active.icon,
            ),
        };

        iced_aw::number_input::Style {
            button_background: Some(iced::Background::Color(background)),
            icon_color: icon,
        }
    }
}

impl iced_aw::style::number_input::ExtendedCatalog for Theme {
    fn style(
        &self,
        class: &<Self as iced_aw::style::number_input::Catalog>::Class<'_>,
        status: iced_aw::card::Status,
    ) -> iced_aw::number_input::Style {
        iced_aw::style::number_input::Catalog::style(self, class, status)
    }
}

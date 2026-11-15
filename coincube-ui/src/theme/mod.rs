pub mod badge;
pub mod banner;
pub mod button;
pub mod card;
pub mod checkbox;
pub mod container;
pub mod context_menu;
pub mod date_picker;
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
pub mod tenshu;
pub mod text;
pub mod text_input;
pub mod toggler;

/// Installation-wide presentation choice, independent of the color scheme.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorldviewMode {
    Suitcoiner,
    Luke,
    #[default]
    #[serde(other)]
    Samurai,
}

/// Decorative artwork choice within one worldview.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtworkStyle {
    Scene,
    #[default]
    #[serde(other)]
    Existing,
}

impl ArtworkStyle {
    pub const fn default_for(worldview: WorldviewMode) -> Self {
        match worldview {
            WorldviewMode::Samurai => Self::Existing,
            WorldviewMode::Suitcoiner | WorldviewMode::Luke => Self::Scene,
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Theme {
    pub colors: palette::Palette,
    pub mode: palette::ThemeMode,
    worldview: WorldviewMode,
    artwork: ArtworkStyle,
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark()
    }
}

impl Theme {
    pub fn dark() -> Self {
        Self {
            colors: palette::Palette::dark(),
            mode: palette::ThemeMode::Dark,
            worldview: WorldviewMode::default(),
            artwork: ArtworkStyle::Existing,
        }
    }

    pub fn light() -> Self {
        Self {
            colors: palette::Palette::light(),
            mode: palette::ThemeMode::Light,
            worldview: WorldviewMode::default(),
            artwork: ArtworkStyle::Existing,
        }
    }

    pub fn from_mode(mode: palette::ThemeMode) -> Self {
        Self {
            colors: palette::Palette::from_mode(mode),
            mode,
            worldview: WorldviewMode::default(),
            artwork: ArtworkStyle::Existing,
        }
    }

    pub fn with_worldview(mut self, worldview: WorldviewMode) -> Self {
        self.worldview = worldview;
        self.artwork = ArtworkStyle::default_for(worldview);
        self
    }

    pub fn with_artwork(mut self, artwork: ArtworkStyle) -> Self {
        self.artwork = artwork;
        self
    }

    pub fn artwork(&self) -> ArtworkStyle {
        self.artwork
    }

    pub fn worldview(&self) -> WorldviewMode {
        self.worldview
    }

    /// The active worldview's accent: muted ink for Samurai, antique gold for
    /// Suitcoiner, oxblood for Luke. Decorative marks only, never body text.
    pub fn worldview_accent(&self) -> iced::Color {
        match self.worldview {
            WorldviewMode::Samurai => self.colors.shell.muted,
            WorldviewMode::Suitcoiner => self.colors.shell.suitcoiner,
            WorldviewMode::Luke => self.colors.shell.luke,
        }
    }
}

impl iced::theme::Base for Theme {
    fn default(_preference: iced::theme::Mode) -> Self {
        <Self as std::default::Default>::default()
    }

    fn mode(&self) -> iced::theme::Mode {
        match self.mode {
            palette::ThemeMode::Dark => iced::theme::Mode::Dark,
            palette::ThemeMode::Light => iced::theme::Mode::Light,
        }
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

impl iced::widget::combo_box::Catalog for Theme {
    fn default_input<'a>() -> <Self as iced::widget::text_input::Catalog>::Class<'a> {
        <Self as iced::widget::text_input::Catalog>::default()
    }

    fn default_menu<'a>() -> <Self as iced::widget::overlay::menu::Catalog>::Class<'a> {
        <Self as iced::widget::overlay::menu::Catalog>::default()
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

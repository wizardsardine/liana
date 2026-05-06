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

/// Generate the boilerplate `pub fn <variant>(&Theme) -> Style` style
/// functions for a theme submodule.
///
/// Most theme submodules (`pill`, `badge`, `banner`, `notification`, parts of
/// `card`, ...) follow the same shape: a private `<builder>(&ContainerPalette)
/// -> Style` constructor, and one public `pub fn <variant>(theme: &Theme) -> Style`
/// per variant that just looks up the matching palette and forwards it to the
/// builder. This macro emits those forwarders.
///
/// # Parameters
///
/// - `$builder`     — ident of the in-scope private builder fn taking the
///   palette and returning a `Style`.
/// - `$palette_group` — ident of the field on [`palette::Palette`] holding the
///   group of variants (e.g. `pills`, `badges`, `banners`).
/// - `[$name, ...]` — bracketed list of variant idents. Each must be both a
///   field on the palette group struct and the name of the
///   public function the macro will generate.
///
/// # Requirements at the call site
///
/// Only the `$builder` fn needs to be in scope (typically defined as a private
/// fn in the same module). `Theme` and `Style` are referenced through absolute
/// paths inside the macro — no extra `use`s required. Just import the macro
/// itself: `use super::styles;`.
///
/// # Example
///
/// ```ignore
/// // In theme/badge.rs
/// use super::styles;
/// use super::palette::ContainerPalette;
///
/// fn badge(palette: &ContainerPalette) -> iced::widget::container::Style { /* ... */ }
///
/// styles!(badge, badges, [simple, bitcoin]);
/// ```
///
/// expands to:
///
/// ```ignore
/// pub fn simple(theme: &crate::theme::Theme) -> iced::widget::container::Style {
///     badge(&theme.colors.badges.simple)
/// }
/// pub fn bitcoin(theme: &crate::theme::Theme) -> iced::widget::container::Style {
///     badge(&theme.colors.badges.bitcoin)
/// }
/// ```
macro_rules! styles {
    ($builder:ident, $palette_group:ident, [$($name:ident),* $(,)?]) => {
        $(
            pub fn $name(theme: &$crate::theme::Theme) -> ::iced::widget::container::Style {
                $builder(&theme.colors.$palette_group.$name)
            }
        )*
    };
}

pub(crate) use styles;

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

impl iced::theme::Base for Theme {
    fn default(_preference: iced::theme::Mode) -> Self {
        <Self as Default>::default()
    }

    fn mode(&self) -> iced::theme::Mode {
        iced::theme::Mode::Light
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
        "Liana"
    }
}

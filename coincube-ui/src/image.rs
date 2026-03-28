use iced::{
    widget::{svg, Svg},
    window::icon,
};

use crate::theme::Theme;
use crate::widget::Row;
use crate::{color, font};

const COINCUBE_LOGOTYPE_GREY: &[u8] = include_bytes!("../static/logos/coincube-logo-gray.svg");

pub fn coincube_window_icon() -> icon::Icon {
    let bytes = include_bytes!("../static/logos/coincube-cc.ico");
    let img = image::load(std::io::Cursor::new(bytes), image::ImageFormat::Ico).unwrap();

    let width = img.width();
    let height = img.height();
    let buffer = img.into_rgba8().into_vec();

    icon::from_rgba(buffer, width, height).unwrap()
}

/// COINCUBE wordmark using Space Grotesk Bold at a given size.
/// "COIN" is always orange; "CUBE" uses the theme's primary text color.
pub fn coincube_wordmark<'a, M: 'a>(size: f32) -> Row<'a, M> {
    use crate::theme;
    iced::widget::row![
        iced::widget::text("COIN")
            .font(font::SPACE_GROTESK_BOLD)
            .size(size)
            .color(color::ORANGE),
        iced::widget::text("CUBE")
            .font(font::SPACE_GROTESK_BOLD)
            .size(size)
            .style(theme::text::primary),
    ]
}

/// Theme toggle button for sidebars. Shows sun/moon icon with "Light Mode"/"Dark Mode" label.
pub fn theme_toggle_button<'a, M: Clone + 'a>(
    mode: crate::theme::palette::ThemeMode,
    on_press: M,
) -> iced::widget::Button<'a, M, Theme, iced::Renderer> {
    use crate::theme::palette::ThemeMode;
    let (icon, label) = match mode {
        ThemeMode::Dark => (crate::icon::sun_icon(), "Light Mode"),
        ThemeMode::Light => (crate::icon::moon_icon(), "Dark Mode"),
    };
    iced::widget::Button::new(
        iced::widget::row![
            icon.style(crate::theme::text::secondary),
            crate::component::text::p2_regular(label),
        ]
        .spacing(8)
        .align_y(iced::alignment::Vertical::Center),
    )
    .on_press(on_press)
    .style(crate::theme::button::transparent)
    .padding([8, 12])
}

/// Grey SVG logotype — used for small badge icons where text rendering is impractical.
pub fn coincube_logotype_grey<'a>() -> Svg<'a, Theme> {
    let h = svg::Handle::from_memory(COINCUBE_LOGOTYPE_GREY);
    Svg::new(h)
}

const CREATE_NEW_WALLET_ICON: &[u8] = include_bytes!("../static/icons/blueprint.svg");

pub fn create_new_wallet_icon<'a>() -> Svg<'a, Theme> {
    let h = svg::Handle::from_memory(CREATE_NEW_WALLET_ICON);
    Svg::new(h)
}

const PARTICIPATE_IN_NEW_WALLET_ICON: &[u8] = include_bytes!("../static/icons/discussion.svg");

pub fn participate_in_new_wallet_icon<'a>() -> Svg<'a, Theme> {
    let h = svg::Handle::from_memory(PARTICIPATE_IN_NEW_WALLET_ICON);
    Svg::new(h)
}

const RESTORE_WALLET_ICON: &[u8] = include_bytes!("../static/icons/syncdata.svg");

pub fn restore_wallet_icon<'a>() -> Svg<'a, Theme> {
    let h = svg::Handle::from_memory(RESTORE_WALLET_ICON);
    Svg::new(h)
}

const SUCCESS_MARK_ICON: &[u8] = include_bytes!("../static/icons/success-mark.svg");

pub fn success_mark_icon<'a>() -> Svg<'a, Theme> {
    let h = svg::Handle::from_memory(SUCCESS_MARK_ICON);
    Svg::new(h)
}

const KEY_MARK_ICON: &[u8] = include_bytes!("../static/icons/key-mark.svg");

pub fn key_mark_icon<'a>() -> Svg<'a, Theme> {
    let h = svg::Handle::from_memory(KEY_MARK_ICON);
    Svg::new(h)
}

const INHERITANCE_TEMPLATE_DESC: &[u8] =
    include_bytes!("../static/images/inheritance_template_description.svg");

pub fn inheritance_template_description<'a>() -> Svg<'a, Theme> {
    let h = svg::Handle::from_memory(INHERITANCE_TEMPLATE_DESC);
    Svg::new(h)
}

const CUSTOM_TEMPLATE_DESC: &[u8] =
    include_bytes!("../static/images/custom_template_description.svg");

pub fn custom_template_description<'a>() -> Svg<'a, Theme> {
    let h = svg::Handle::from_memory(CUSTOM_TEMPLATE_DESC);
    Svg::new(h)
}

const MULTISIG_SECURITY_TEMPLATE_DESC: &[u8] =
    include_bytes!("../static/images/multisig_security_template.svg");

pub fn multisig_security_template_description<'a>() -> Svg<'a, Theme> {
    let h = svg::Handle::from_memory(MULTISIG_SECURITY_TEMPLATE_DESC);
    Svg::new(h)
}

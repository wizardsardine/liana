use iced::{
    widget::{svg, Svg},
    window::icon,
};

use crate::theme::Theme;
use crate::widget::Row;
use crate::{color, font};

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

/// COINCUBE wordmark in gray tones using Space Grotesk Bold.
/// "COIN" is medium gray; "CUBE" is lighter gray.
pub fn coincube_wordmark_gray<'a, M: 'a>(size: f32) -> Row<'a, M> {
    iced::widget::row![
        iced::widget::text("COIN")
            .font(font::SPACE_GROTESK_BOLD)
            .size(size)
            .color(color::GREY_3),
        iced::widget::text("CUBE")
            .font(font::SPACE_GROTESK_BOLD)
            .size(size)
            .color(color::GREY_2),
    ]
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

// ---------------------------------------------------------------------------
// USDt + network logos
// ---------------------------------------------------------------------------

const BTC_LOGO: &[u8] = include_bytes!("../static/logos/btc.svg");
const LBTC_LOGO: &[u8] = include_bytes!("../static/logos/lbtc.svg");
const USDT_LOGO: &[u8] = include_bytes!("../static/logos/usdt.svg");
const ETH_LOGO: &[u8] = include_bytes!("../static/logos/eth.svg");
const TRX_LOGO: &[u8] = include_bytes!("../static/logos/trx.svg");
const BNB_LOGO: &[u8] = include_bytes!("../static/logos/bnb.svg");
const SOL_LOGO: &[u8] = include_bytes!("../static/logos/sol.svg");
const LIQUID_LOGO: &[u8] = include_bytes!("../static/logos/liquid.svg");
const LIGHTNING_BADGE: &[u8] = include_bytes!("../static/logos/lightning_badge.svg");
const CHAIN_BADGE: &[u8] = include_bytes!("../static/logos/chain_badge.svg");

pub fn btc_logo<'a>() -> Svg<'a, Theme> {
    Svg::new(svg::Handle::from_memory(BTC_LOGO))
}

pub fn lbtc_logo<'a>() -> Svg<'a, Theme> {
    Svg::new(svg::Handle::from_memory(LBTC_LOGO))
}

pub fn usdt_logo<'a>() -> Svg<'a, Theme> {
    Svg::new(svg::Handle::from_memory(USDT_LOGO))
}

pub fn lightning_badge<'a>() -> Svg<'a, Theme> {
    Svg::new(svg::Handle::from_memory(LIGHTNING_BADGE))
}

pub fn chain_badge<'a>() -> Svg<'a, Theme> {
    Svg::new(svg::Handle::from_memory(CHAIN_BADGE))
}

pub fn eth_logo<'a>() -> Svg<'a, Theme> {
    Svg::new(svg::Handle::from_memory(ETH_LOGO))
}

pub fn trx_logo<'a>() -> Svg<'a, Theme> {
    Svg::new(svg::Handle::from_memory(TRX_LOGO))
}

pub fn bnb_logo<'a>() -> Svg<'a, Theme> {
    Svg::new(svg::Handle::from_memory(BNB_LOGO))
}

pub fn sol_logo<'a>() -> Svg<'a, Theme> {
    Svg::new(svg::Handle::from_memory(SOL_LOGO))
}

pub fn liquid_logo<'a>() -> Svg<'a, Theme> {
    Svg::new(svg::Handle::from_memory(LIQUID_LOGO))
}

/// Returns the network logo SVG for a given network slug.
pub fn network_logo<'a>(network: &str) -> Svg<'a, Theme> {
    match network {
        "ethereum" => eth_logo(),
        "tron" => trx_logo(),
        "bsc" => bnb_logo(),
        "solana" => sol_logo(),
        "liquid" => liquid_logo(),
        "polygon" => eth_logo(),
        "lightning" => lightning_badge(),
        "bitcoin" => chain_badge(),
        _ => usdt_logo(),
    }
}

/// Returns the asset logo SVG for a given asset slug.
pub fn asset_logo<'a>(asset: &str) -> Svg<'a, Theme> {
    match asset {
        "btc" | "lbtc" => btc_logo(),
        "usdt" => usdt_logo(),
        _ => btc_logo(),
    }
}

/// Composite USDt logo with a smaller network badge at the bottom-right.
///
/// Layout: USDt logo at `primary_size`, with the network logo overlaid at
/// ~40% size, anchored to the bottom-right corner.
///
/// ```text
///  ┌──────────┐
///  │  USDt    │
///  │      ┌──┐│
///  │      │NW││
///  └──────┴──┘┘
/// ```
pub fn usdt_network_logo<'a, M: 'a>(
    network: &str,
    primary_size: f32,
) -> iced::Element<'a, M, Theme, iced::Renderer> {
    use iced::{widget::Container, Length};

    let badge_size = (primary_size * 0.45).round();
    let total = primary_size;

    let primary = usdt_logo()
        .width(Length::Fixed(primary_size))
        .height(Length::Fixed(primary_size));

    let badge = network_logo(network)
        .width(Length::Fixed(badge_size))
        .height(Length::Fixed(badge_size));

    // Stack: primary logo fills the area; badge is placed at bottom-right
    // using iced's overlay / stack isn't available, so we use a layered approach
    // with negative margin via padding on an outer container.
    let badge_container = Container::new(badge)
        .width(Length::Fixed(badge_size))
        .height(Length::Fixed(badge_size));

    // Use iced::widget::stack to overlay the badge on the primary logo
    iced::widget::stack![
        Container::new(primary)
            .width(Length::Fixed(total))
            .height(Length::Fixed(total)),
        Container::new(badge_container)
            .width(Length::Fixed(total))
            .height(Length::Fixed(total))
            .align_right(Length::Fixed(total))
            .align_bottom(Length::Fixed(total)),
    ]
    .width(Length::Fixed(total))
    .height(Length::Fixed(total))
    .into()
}

/// Composite any asset logo with a network badge at the bottom-right.
///
/// `asset` — slug for the primary logo: "btc", "lbtc", "usdt"
/// `network` — slug for the badge: "lightning", "liquid", "bitcoin",
///             "ethereum", "tron", "bsc", "solana"
pub fn asset_network_logo<'a, M: 'a>(
    asset: &str,
    network: &str,
    primary_size: f32,
) -> iced::Element<'a, M, Theme, iced::Renderer> {
    use iced::{widget::Container, Length};

    let badge_size = (primary_size * 0.45).round();
    let total = primary_size;

    let primary = asset_logo(asset)
        .width(Length::Fixed(primary_size))
        .height(Length::Fixed(primary_size));

    let badge = network_logo(network)
        .width(Length::Fixed(badge_size))
        .height(Length::Fixed(badge_size));

    let badge_container = Container::new(badge)
        .width(Length::Fixed(badge_size))
        .height(Length::Fixed(badge_size));

    iced::widget::stack![
        Container::new(primary)
            .width(Length::Fixed(total))
            .height(Length::Fixed(total)),
        Container::new(badge_container)
            .width(Length::Fixed(total))
            .height(Length::Fixed(total))
            .align_right(Length::Fixed(total))
            .align_bottom(Length::Fixed(total)),
    ]
    .width(Length::Fixed(total))
    .height(Length::Fixed(total))
    .into()
}

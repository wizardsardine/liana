//! The Launcher's top bar, dock and Key card, from P-Launcher's `.top`,
//! `.bottom` and `.keycard`. Sizes, weights and colours follow the
//! prototype's CSS; every control keeps the catalog's existing ids and
//! messages, so focus, the network gate and the quota stay authoritative.

use coincube_ui::locale::copy as localized_copy;
use coincube_ui::localized_format;
use iced::{
    advanced::widget::Id,
    widget::{button as iced_button, container, mouse_area, svg, text, Space},
    Alignment, Border, Color, Length, Padding,
};

use coincube_ui::{
    component::tenshu::top_bar::{self, APP_PREFERENCES, SLIDERS},
    font, theme,
    widget::{action::Action, Column, Container, Element, Row},
};

use super::{
    launcher_activity::{self, ActivityMessage},
    launcher_catalog::{self, CatalogMessage, KeyPart, NetworkFilter, Projection},
    launcher_coin, launcher_scene, Home, HomeSection, ViewMessage,
};
use crate::{
    app::settings::preferences::{NetworkActivitySource, PreferenceChange},
    chain::{ChainId, ChainIdExt},
    feature_flags,
};

/// `.top` and `.bottom`: 64px bars with a 24px gutter, as the shared top bar.
pub(super) const BAR_HEIGHT: f32 = top_bar::HEIGHT;
const GUTTER: f32 = top_bar::GUTTER;
/// `.keycard`: 300px wide with one column, 440px with the Connect column,
/// 8px above the bar.
const KEY_CARD_WIDTH: f32 = 300.0;
const KEY_CARD_WIDE: f32 = 440.0;
/// The Key's face column: "Right" over "lock or kit".
const KEY_FACE_WIDTH: f32 = 72.0;
/// `.keybtn { height: 32px }`.
const KEY_BUTTON_HEIGHT: f32 = 32.0;
const ACTIVITY_MENU_WIDTH: f32 = 300.0;
/// The Key button's width, so the menus beside it open over their own
/// buttons rather than the Key's.
const KEY_BUTTON_WIDTH: f32 = 72.0;
/// The Network activity button's width, for the same reason.
const ACTIVITY_BUTTON_WIDTH: f32 = 172.0;
const PLAN_MENU_WIDTH: f32 = 320.0;

const PERSON: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#fff" stroke-width="1.5" stroke-linecap="round"><circle cx="12" cy="8.5" r="3.5"/><path d="M5 20c.8-3.6 3.6-5.5 7-5.5s6.2 1.9 7 5.5"/></svg>"##;
const PLUS: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#fff" stroke-width="1.5" stroke-linecap="round"><path d="M12 5v14M5 12h14"/></svg>"##;
const IMPORT: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#fff" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M12 4v11M7.5 10.5L12 15l4.5-4.5M5 19h14"/></svg>"##;
const KEY: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#fff" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><circle cx="8" cy="15" r="4"/><path d="M11 12l8-8M16 7l2 2M14 9l2 2"/></svg>"##;
const SHIELD: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#fff" stroke-width="1.5" stroke-linejoin="round"><path d="M12 3l7 3v5.5c0 4.3-3 7.6-7 9.5-4-1.9-7-5.2-7-9.5V6z"/></svg>"##;
/// An imported wallet: a billfold, since a coin would read as the network
/// (Robert, 2026-11-07).
const WALLET: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#fff" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M4 7.5h15a1 1 0 0 1 1 1V19a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1zM4 7.5 15 4.2v3.3M16 14h1.5"/></svg>"##;
const CLOUD: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#fff" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M7 18h10a4 4 0 0 0 .6-7.95A5.5 5.5 0 0 0 6.9 9.2 4.4 4.4 0 0 0 7 18z"/></svg>"##;
const KIT: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#fff" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M4 8.5h16V19H4zM9 8.5V6.5a1 1 0 0 1 1-1h4a1 1 0 0 1 1 1v2M8.8 13.6l2.2 2.2 4.2-4.3"/></svg>"##;
const NOKIT: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#fff" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M4 8.5h16V19H4zM9 8.5V6.5a1 1 0 0 1 1-1h4a1 1 0 0 1 1 1v2M3 4l18 18"/></svg>"##;
/// Spark's asterisk from `coincube-ui`'s `spark_badge.svg`, filled and fitted
/// to the 24-unit grid like the face mark.
const SPARK: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path transform="translate(3 3.4) scale(0.2675)" fill="#fff" fill-rule="evenodd" d="M39.7159 25.248L40.8727 0.570312H26.4219L27.5787 25.2483L4.46555 16.5221L0 30.2656L23.8282 36.7915L8.38717 56.0763L20.0781 64.5703L33.6483 43.9245L47.2179 64.5695L58.9089 56.0755L43.4679 36.7909L67.2937 30.2657L62.8281 16.5221L39.7159 25.248Z"/></svg>"##;
const LOCK: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#fff" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M7 11h10v9H7zM9 11V8a3 3 0 0 1 6 0v3"/></svg>"##;
const PASSKEY: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#fff" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M9 5a4 4 0 1 0 0 8a4 4 0 1 0 0-8M12 11.5l7 7m-3-3 2.5-2.5m-.5 5 2.5-2.5"/></svg>"##;
const CHEVRON: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#fff" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M7 10l5 5 5-5"/></svg>"##;
const TICK: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#fff" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><path d="M5 12.5l4.5 4.5L19 7.5"/></svg>"##;
const ATTENTION: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#fff" stroke-width="1.5" stroke-linecap="round"><path d="M12 3a9 9 0 1 0 0 18a9 9 0 1 0 0-18M12 7.5v5.5M12 16.2v.3"/></svg>"##;
const QUESTION: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#fff" stroke-width="1.5" stroke-linecap="round"><path d="M12 3a9 9 0 1 0 0 18a9 9 0 1 0 0-18M9.6 9.6a2.4 2.4 0 1 1 3.4 2.2c-.7.3-1 .8-1 1.5v.4M12 16.3v.3"/></svg>"##;

#[derive(Debug, Clone, Copy)]
pub(super) enum Ink {
    Body,
    Muted,
    Accent,
    /// `--warn`: a backup that needs attention.
    Warn,
    /// `--good`: a Recovery Kit that restores the Cube here.
    Good,
    /// `--on-accent`, text and icons on the primary fill.
    OnAccent,
    /// Lays out like the bar's brand without drawing it, to align a menu.
    /// Text only: Iced's SVG tint ignores alpha.
    Clear,
    /// A disabled dock action's content: the muted ink, faded like its border.
    Faded,
    /// A disabled primary action's content on its faded fill.
    FadedOnAccent,
}

/// `.quiet` mutes the bar text while a Cube's face is up.
fn body(quiet: bool) -> Ink {
    if quiet {
        Ink::Muted
    } else {
        Ink::Body
    }
}

fn ink(theme: &theme::Theme, tone: Ink) -> Color {
    let shell = theme.colors.shell;
    match tone {
        Ink::Body => shell.ink,
        Ink::Muted => shell.muted,
        Ink::Accent => shell.accent_ink,
        Ink::Warn => shell.warn,
        Ink::Good => shell.good,
        Ink::OnAccent => iced::color!(0x201407),
        Ink::Clear => Color::TRANSPARENT,
        // Solid blends toward the dock's rail: Iced's SVG tint ignores alpha,
        // and `dock_button` fades a disabled fill and border to 45%.
        Ink::Faded => blend(shell.muted, shell.rail, 0.55),
        Ink::FadedOnAccent => blend(iced::color!(0x201407), shell.rail, 0.55),
    }
}

/// `from` moved `toward` another colour by `amount` (0 to 1), opaque.
fn blend(from: Color, toward: Color, amount: f32) -> Color {
    let mix = |a: f32, b: f32| a + (b - a) * amount;
    Color::from_rgb(
        mix(from.r, toward.r),
        mix(from.g, toward.g),
        mix(from.b, toward.b),
    )
}

/// A prototype stroke icon, tinted like `currentColor`.
fn glyph<'a, M: 'a>(source: &'static str, size: f32, tone: Ink) -> Element<'a, M> {
    svg(svg::Handle::from_memory(source.as_bytes()))
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .style(move |theme: &theme::Theme, _| svg::Style {
            color: Some(ink(theme, tone)),
        })
        .into()
}

fn words<'a, M: 'a>(
    value: impl Into<String>,
    size: f32,
    weight: iced::Font,
    tone: Ink,
) -> Element<'a, M> {
    text(value.into())
        .size(size)
        .font(weight)
        .style(move |theme: &theme::Theme| text::Style {
            color: Some(ink(theme, tone)),
        })
        .into()
}

fn focus(id: Id) -> ViewMessage {
    ViewMessage::Catalog(CatalogMessage::Focus(id))
}

/// `.tbtn`: a 32px borderless bar control whose border appears on hover.
fn bar_button(
    theme: &theme::Theme,
    status: iced_button::Status,
    pressed: bool,
) -> iced_button::Style {
    top_bar::button_style(theme, status, pressed)
}

/// `.btn` and `.btn.primary`: 36px dock actions, 8px radius.
fn dock_button(
    theme: &theme::Theme,
    status: iced_button::Status,
    primary: bool,
) -> iced_button::Style {
    let shell = theme.colors.shell;
    let hovered = matches!(
        status,
        iced_button::Status::Hovered | iced_button::Status::Pressed
    );
    let disabled = matches!(status, iced_button::Status::Disabled);
    let fade = |c: Color| {
        if disabled {
            Color { a: c.a * 0.45, ..c }
        } else {
            c
        }
    };
    if primary {
        let fill = if hovered {
            iced::color!(0xffa54d)
        } else {
            shell.accent
        };
        iced_button::Style {
            background: Some(iced::Background::Color(fade(fill))),
            text_color: fade(iced::color!(0x201407)),
            border: Border {
                color: fade(fill),
                width: 1.0,
                radius: 8.0.into(),
            },
            ..Default::default()
        }
    } else {
        iced_button::Style {
            background: None,
            text_color: fade(shell.ink),
            border: Border {
                color: fade(if hovered { shell.muted } else { shell.line }),
                width: 1.0,
                radius: 8.0.into(),
            },
            ..Default::default()
        }
    }
}

/// A network's full name (`CH.full`), for a Cube's face and its unlock.
pub(crate) fn network_name(chain: ChainId) -> &'static str {
    match chain {
        ChainId::Bitcoin => "Bitcoin mainnet",
        ChainId::BitcoinBlake2b => "Bitcoin Blake2b mainnet",
        ChainId::Testnet => "Bitcoin Testnet",
        ChainId::Testnet4 => "Bitcoin Testnet4",
        ChainId::Signet => "Bitcoin Signet",
        ChainId::Regtest => "Bitcoin Regtest",
        ChainId::BitcoinBlake2bTestnet4 => "Bitcoin Blake2b Testnet4",
    }
}

/// A Network menu row's count: this device's Cubes, then the network's
/// Connect-only Cubes when it has any ("5 + 2 in Connect"), so the menu adds
/// up to the per-network limit the dock and the Create gate count.
fn network_menu_count(projection: &Projection<'_>, chain: ChainId, local: usize) -> String {
    if projection.unread.contains(&chain) {
        return "Unavailable".to_string();
    }
    let remote = projection
        .usage
        .iter()
        .find(|usage| usage.chain == chain)
        .map_or(0, |usage| usage.remote);
    if remote > 0 {
        format!("{local} + {remote} in Connect")
    } else {
        local.to_string()
    }
}

/// A network's menu name (`CH.menu`), for the Network chip and menu.
pub(super) fn network_menu_name(chain: ChainId) -> &'static str {
    match chain {
        ChainId::Bitcoin => "Bitcoin mainnet",
        ChainId::BitcoinBlake2b => "Bitcoin Blake2b mainnet",
        ChainId::BitcoinBlake2bTestnet4 => "Blake2b Testnet4",
        other => other.label(),
    }
}

/// `.chip`: 32px, square-cornered; `.chip.on` while a network narrows the Cubes shown.
fn chip_style(on: bool) -> impl Fn(&theme::Theme, iced_button::Status) -> iced_button::Style {
    move |theme, status| {
        let shell = theme.colors.shell;
        let hovered = matches!(
            status,
            iced_button::Status::Hovered | iced_button::Status::Pressed
        );
        iced_button::Style {
            background: on.then_some(iced::Background::Color(shell.selected)),
            text_color: shell.ink,
            border: Border {
                color: if on {
                    shell.accent
                } else if hovered {
                    shell.muted
                } else {
                    shell.line
                },
                width: 1.0,
                radius: 8.0.into(),
            },
            ..Default::default()
        }
    }
}

/// The `.menu` row for one network.
pub(super) fn network_option_id(chain: ChainId) -> Id {
    Id::from(format!("tenshu.launcher.network.{chain:?}"))
}

fn connect_entry<'a>(home: &'a Home, quiet: bool) -> Element<'a, ViewMessage> {
    connect_control(home, quiet, launcher_catalog::connect_id(), true)
}

/// What the Launcher's Connect control shows, from the account alone: the
/// Launcher has no live updates, so its light says whether the session is
/// signed in, being checked, waiting for the network or Connect, or ended by
/// Connect.
pub(super) fn launcher_connect_state(
    home: &Home,
) -> (
    crate::app::view::shared::connect_control::ConnectDetail<'_>,
    Option<crate::app::ConnectLight>,
    String,
) {
    use crate::app::view::shared::connect_control::ConnectDetail;
    use crate::app::ConnectLight;
    use crate::services::connect::grpc::trouble::Trouble;
    let account = &home.connect_account;
    if account.is_authenticated() {
        let email = account
            .user
            .as_ref()
            .map(|u| u.email.as_str())
            .filter(|e| !e.is_empty());
        let label = match email {
            Some(email) => format!("Connect account, {email}, signed in"),
            None => "Connect account, signed in".to_owned(),
        };
        return (
            email.map_or(ConnectDetail::Account("Signed in"), ConnectDetail::Account),
            Some(ConnectLight::On),
            label,
        );
    }
    if let Some(unverified) = account.unverified.as_ref() {
        return match unverified.trouble {
            Trouble::Offline => (
                ConnectDetail::Offline,
                Some(ConnectLight::Offline),
                format!(
                    "Connect account, {}, offline: no internet connection",
                    unverified.email
                ),
            ),
            Trouble::Unreachable => (
                ConnectDetail::Account(unverified.email.as_str()),
                Some(ConnectLight::Reconnecting),
                format!("Connect account, {}, reconnecting", unverified.email),
            ),
        };
    }
    if account.is_checking_session() {
        return (
            ConnectDetail::Checking,
            Some(ConnectLight::Connecting),
            "Connect account, checking your session".to_owned(),
        );
    }
    match account.signed_out_by {
        Some(reason) => (
            ConnectDetail::SignIn,
            Some(ConnectLight::NeedsYou),
            match reason {
                crate::app::SignedOutBy::ComputerRemoved => "Connect: Sign in, needs attention: \
                     this computer was removed from your Connect account"
                    .to_owned(),
                crate::app::SignedOutBy::SessionEnded => {
                    "Connect: Sign in, needs attention: your Connect session ended".to_owned()
                }
            },
        ),
        None => (ConnectDetail::SignIn, None, "Connect: Sign in".to_owned()),
    }
}

/// The Launcher's Connect control under the caller's focus id, so another
/// bar (Create Cube) draws the same control: the one an open Cube draws,
/// without a panel. It keeps one id in every state (L6). `enabled` gates the
/// press on top of the control's own rule; a quiet bar takes no input.
pub(super) fn connect_control<'a>(
    home: &'a Home,
    quiet: bool,
    id: Id,
    enabled: bool,
) -> Element<'a, ViewMessage> {
    let overview = ViewMessage::GoToSection(HomeSection::Connect(
        crate::app::menu::ConnectSubMenu::Overview,
    ));
    let (detail, light, label) = launcher_connect_state(home);
    // Inert while the session is first checked, as before.
    let pressable = enabled
        && !quiet
        && detail != crate::app::view::shared::connect_control::ConnectDetail::Checking;
    crate::app::view::shared::connect_control::connect_control(
        crate::app::view::shared::connect_control::ConnectControl {
            id,
            detail,
            light,
            light_dim: home.connect_light_dim,
            chevron: false,
            open: false,
            label,
        },
        pressable.then_some(overview),
        focus,
    )
}

/// One `.chip`: a 32px filter toggle, `.chip.on` while it applies.
fn filter_chip<'a>(
    id: Id,
    label: String,
    on: bool,
    quiet: bool,
    menu: bool,
    message: ViewMessage,
) -> Element<'a, ViewMessage> {
    let on = on && !quiet;
    let mut content = Row::new().spacing(6).align_y(Alignment::Center).push(words(
        label,
        13.0,
        if on { font::MEDIUM } else { font::REGULAR },
        body(quiet),
    ));
    if menu {
        content = content.push(glyph(CHEVRON, 14.0, body(quiet)));
    }
    Action::new(id, Container::new(content).center_y(Length::Fill), focus)
        .height(Length::Fixed(32.0))
        .padding([0.0, 12.0])
        .style(chip_style(on))
        .on_press(message)
        .into()
}

/// The chips that narrow the Launcher by network, as on the canvas: "All
/// networks" when more than one network is listed, "Live" and "Test" in
/// Developer mode, then the Network menu.
fn filter_chips<'a>(home: &Home, filter: NetworkFilter, quiet: bool) -> Row<'a, ViewMessage> {
    let pick = |filter| ViewMessage::Catalog(CatalogMessage::Filter(filter));
    let mut chips = Row::new().spacing(8).align_y(Alignment::Center);
    for (filter_kind, label, id) in group_chips(home) {
        chips = chips.push(filter_chip(
            launcher_catalog::filter_id(id),
            label.to_string(),
            filter == filter_kind,
            quiet,
            false,
            pick(filter_kind),
        ));
    }
    let (label, on) = match filter {
        NetworkFilter::Network(chain) => (format!("Network: {}", network_menu_name(chain)), true),
        _ => ("Network".to_string(), false),
    };
    chips.push(filter_chip(
        launcher_catalog::network_id(),
        label,
        on,
        quiet,
        true,
        ViewMessage::Catalog(CatalogMessage::NetworkMenu(!home.catalog.network_menu)),
    ))
}

/// The Network menu, placed under the Network chip: undrawn copies of the
/// All / Live / Test chips take their widths and their 8px gaps.
fn under_network_chip<'a>(home: &Home, menu: Element<'a, ViewMessage>) -> Row<'a, ViewMessage> {
    let mut row = Row::new().spacing(8);
    for (_, label, _) in group_chips(home) {
        row = row.push(
            Container::new(words(label, 13.0, font::REGULAR, Ink::Clear))
                .height(Length::Fixed(32.0))
                .padding([0.0, 12.0]),
        );
    }
    row.push(menu)
}

/// The All / Live / Test chips this Home shows.
fn group_chips(home: &Home) -> Vec<(NetworkFilter, &'static str, &'static str)> {
    let mut chips = Vec::new();
    if home.displayed_networks.len() > 1 {
        chips.push((NetworkFilter::All, "All networks", "all"));
    }
    if home.developer_mode() {
        chips.push((NetworkFilter::Live, "Live networks", "live"));
        chips.push((NetworkFilter::Test, "Test networks", "test"));
    }
    chips
}

/// `.linkbtn` "Clear filters": 13px ink with the accent underline.
fn clear_filters<'a>(id: Id) -> Element<'a, ViewMessage> {
    Action::new(
        id,
        iced::widget::Stack::new()
            .push(
                Column::new()
                    .push(words(
                        localized_copy("Clear filters"),
                        13.0,
                        font::REGULAR,
                        Ink::Body,
                    ))
                    .push(Space::new().height(Length::Fixed(3.0))),
            )
            .push(
                Column::new()
                    .height(Length::Fill)
                    .push(Space::new().height(Length::Fill))
                    .push(
                        Container::new(Space::new())
                            .width(Length::Fill)
                            .height(Length::Fixed(1.0))
                            .style(|theme: &theme::Theme| container::Style {
                                background: Some(iced::Background::Color(
                                    theme.colors.shell.accent,
                                )),
                                ..Default::default()
                            }),
                    ),
            ),
        focus,
    )
    .padding([2.0, 4.0])
    .style(|_theme: &theme::Theme, _status| iced_button::Style::default())
    .on_press(ViewMessage::Catalog(CatalogMessage::Filter(
        NetworkFilter::All,
    )))
    .into()
}

/// `.vsep`: the 1px rule between the brand and the filters.
fn separator<'a, M: 'a>() -> Element<'a, M> {
    Container::new(Space::new())
        .width(Length::Fixed(1.0))
        .height(Length::Fixed(20.0))
        .style(|theme: &theme::Theme| container::Style {
            background: Some(iced::Background::Color(theme.colors.shell.line)),
            ..Default::default()
        })
        .into()
}

/// `.top` on the shared bar: brand, network chips and count; then balances,
/// Connect and App preferences. The theme lives in App preferences, as in the
/// prototype.
pub(super) fn top_bar<'a>(
    home: &'a Home,
    projection: &Projection<'_>,
    quiet: bool,
) -> Element<'a, ViewMessage> {
    let count = count_text(
        projection.locals.len(),
        projection.local_total,
        projection.connect_only(home.catalog.mode),
    );
    let preferences = top_bar::preferences(
        launcher_catalog::settings_id(),
        focus,
        ViewMessage::OpenAppPreferences,
    )
    .pressed(home.app_preferences_open)
    .muted(quiet);
    top_bar::top_bar(top_bar::brand_mark(quiet), preferences)
        .lead(separator())
        .lead(filter_chips(home, projection.filter, quiet))
        .lead(words(count, 13.0, font::REGULAR, Ink::Muted))
        .lead(
            (projection.filter != NetworkFilter::All)
                .then(|| clear_filters(launcher_catalog::clear_filters_id())),
        )
        .chip(connect_entry(home, quiet))
        .into()
}

fn rail(theme: &theme::Theme) -> container::Style {
    container::Style {
        background: Some(iced::Background::Color(theme.colors.shell.rail)),
        text_color: Some(theme.colors.shell.ink),
        ..Default::default()
    }
}

fn dock_action<'a>(
    id: Id,
    icon: &'static str,
    label: &'static str,
    primary: bool,
    message: Option<ViewMessage>,
    quiet: bool,
) -> Element<'a, ViewMessage> {
    // `.quiet .dock .btn.primary` drops the fill; every label goes muted.
    // A disabled action fades its border (`dock_button`) and its content, so
    // it reads as unavailable rather than as an outlined button.
    let primary = primary && !quiet;
    let enabled = message.is_some();
    let tone = match (primary, enabled) {
        (true, true) => Ink::OnAccent,
        (true, false) => Ink::FadedOnAccent,
        (false, true) => body(quiet),
        (false, false) => Ink::Faded,
    };
    Action::new(
        id,
        Container::new(
            Row::new()
                .spacing(8)
                .align_y(Alignment::Center)
                .push(glyph(icon, 16.0, tone))
                .push(words(label, 14.0, font::MEDIUM, tone)),
        )
        .center_y(Length::Fill),
        focus,
    )
    .height(Length::Fixed(36.0))
    .padding([0.0, 16.0])
    .style(move |theme, status| dock_button(theme, status, primary))
    .on_press_maybe(message)
    .into()
}

/// "[network]: [N] of [N] Cubes on the [plan] plan" while the plan has no
/// Cube slot left on the current network: Create Cube and Import or restore
/// are both closed then. The limit is per network, so the copy names it —
/// a bare ratio under "All networks" reads as a total across every network.
pub(super) fn plan_full(projection: &Projection<'_>) -> Option<String> {
    projection.at_limit().then(|| {
        if projection.total_count > projection.cube_limit {
            // Over the limit after a plan change or the billing v2 limits:
            // existing Cubes keep working, only new ones are blocked.
            format!(
                "{}: {} Cubes; the {} plan includes {}. Existing Cubes keep working.",
                network_name(projection.network),
                projection.total_count,
                projection.tier,
                projection.cube_limit
            )
        } else {
            format!(
                "{}: {} of {} Cubes on the {} plan",
                network_name(projection.network),
                projection.total_count,
                projection.cube_limit,
                projection.tier
            )
        }
    })
}

fn small_link<'a>(id: Id, label: &'static str, message: ViewMessage) -> Element<'a, ViewMessage> {
    Action::new(id, words(label, 13.0, font::MEDIUM, Ink::Accent), focus)
        .padding([4.0, 6.0])
        .style(|theme, status| bar_button(theme, status, false))
        .on_press(message)
        .into()
}

/// `.bottom`: Create and Import on the left; the plan, Network activity and
/// Key on the right. The app's gated Recover entry shares the same 64px bar.
pub(super) fn dock<'a>(
    home: &Home,
    projection: &Projection<'_>,
    quiet: bool,
) -> Element<'a, ViewMessage> {
    // Both entries make a Cube, so both close only when the plan has no
    // Cube slot left; the reason and remedy then read inline beside them.
    let full = plan_full(projection);
    let create = dock_action(
        launcher_catalog::create_id(),
        PLUS,
        "Create Cube",
        true,
        full.is_none().then_some(ViewMessage::ShowCreateCube(true)),
        quiet,
    );
    let import = dock_action(
        launcher_catalog::import_id(),
        IMPORT,
        "Import or restore Bitcoin wallet",
        false,
        full.is_none().then_some(ViewMessage::ShowRestoreHub),
        quiet,
    );
    let import_caption = full.map(|caption| {
        Row::new()
            .spacing(4)
            .align_y(Alignment::Center)
            .push(words(caption, 12.0, font::REGULAR, Ink::Muted))
            .push(small_link(
                Id::new("tenshu.launcher.dock.upgrade"),
                "Upgrade plan",
                ViewMessage::GoToSection(HomeSection::Connect(
                    crate::app::menu::ConnectSubMenu::PlanBilling,
                )),
            ))
    });
    let recover = (home.connect_account.is_authenticated()
        && !home.network.is_blake2b()
        && feature_flags::RECOVER_VAULT_ENABLED)
        .then(|| {
            small_link(
                launcher_catalog::recover_vault_id(),
                "Recover a Vault",
                ViewMessage::GoToSection(HomeSection::RecoverVault),
            )
        });
    let key = (projection.filtered_total > 0).then(|| key_button(home.catalog.key_open()));
    Container::new(
        Row::new()
            .spacing(8)
            .align_y(Alignment::Center)
            .height(Length::Fill)
            .push(create)
            .push(import)
            .push(import_caption)
            .push(recover)
            .push(Space::new().width(Length::Fill))
            .push(plan_button(home, projection))
            .push(activity_button(home))
            .push(key),
    )
    .height(Length::Fixed(BAR_HEIGHT))
    .padding([0.0, GUTTER])
    .width(Length::Fill)
    .style(rail)
    .into()
}

fn activity_id() -> Id {
    Id::new("tenshu.launcher.activity")
}

fn activity_option_id(source: NetworkActivitySource) -> Id {
    Id::new(match source {
        NetworkActivitySource::Automatic => "tenshu.launcher.activity.automatic",
        NetworkActivitySource::LocalNode => "tenshu.launcher.activity.local",
        NetworkActivitySource::Connect => "tenshu.launcher.activity.connect",
    })
}

/// A 32px bar control that shows `open` while its menu is up.
fn menu_button_style(
    open: bool,
) -> impl Fn(&theme::Theme, iced_button::Status) -> iced_button::Style {
    move |theme, status| {
        let mut style = bar_button(theme, status, false);
        if open {
            style.background = Some(iced::Background::Color(theme.colors.shell.panel));
            style.border.color = theme.colors.shell.line;
        }
        style
    }
}

/// "Network activity", with a dot in the shown network's edge colour once a
/// read succeeds. The block, its source and any failure are in the menu: the
/// sky above the Cubes already shows the block.
fn activity_button<'a>(home: &Home) -> Option<Element<'a, ViewMessage>> {
    if !home.activity_wanted() {
        return None;
    }
    let chain = home.activity_chain();
    let light = matches!(
        home.theme_mode,
        coincube_ui::theme::palette::ThemeMode::Light
    );
    let dot = matches!(home.catalog.activity.current(chain), Some(Ok(_)))
        .then(|| launcher_scene::edge(chain, light));
    let open = home.catalog.activity.menu;
    let dot = Container::new(Space::new())
        .width(Length::Fixed(8.0))
        .height(Length::Fixed(8.0))
        .style(move |theme: &theme::Theme| container::Style {
            background: dot.map(|c| {
                iced::Background::Color(Color::from_rgba8(
                    c[0],
                    c[1],
                    c[2],
                    f32::from(c[3]) / 255.0,
                ))
            }),
            border: Border {
                color: theme.colors.shell.line,
                width: if dot.is_some() { 0.0 } else { 1.0 },
                radius: 2.0.into(),
            },
            ..Default::default()
        });
    Some(
        Action::new(
            activity_id(),
            Container::new(
                Row::new()
                    .spacing(8)
                    .align_y(Alignment::Center)
                    .push(dot)
                    .push(words(
                        localized_copy("Network activity"),
                        13.0,
                        font::REGULAR,
                        Ink::Body,
                    ))
                    .push(Space::new().width(Length::Fill))
                    .push(glyph(CHEVRON, 14.0, Ink::Muted)),
            )
            .center_y(Length::Fill),
            focus,
        )
        .width(Length::Fixed(ACTIVITY_BUTTON_WIDTH))
        .height(Length::Fixed(KEY_BUTTON_HEIGHT))
        .padding([0.0, 10.0])
        .style(menu_button_style(open))
        .on_press(ViewMessage::Catalog(CatalogMessage::Activity(
            ActivityMessage::Menu(!open),
        )))
        .into(),
    )
}

fn activity_network_id(blake2b: bool) -> Id {
    Id::new(if blake2b {
        "tenshu.launcher.activity.network.blake2b"
    } else {
        "tenshu.launcher.activity.network.bitcoin"
    })
}

/// The menu's Bitcoin / Bitcoin Blake2b switch, two halves of one control.
fn activity_networks<'a>(home: &Home) -> Element<'a, ViewMessage> {
    let chosen = home.activity_chain().is_blake2b();
    let half = |blake2b: bool, label: &'static str| {
        let on = blake2b == chosen;
        Action::new(
            activity_network_id(blake2b),
            Container::new(words(
                label,
                13.0,
                if on { font::SEMIBOLD } else { font::REGULAR },
                if on { Ink::Body } else { Ink::Muted },
            ))
            .center_x(Length::Fill)
            .center_y(Length::Fill),
            focus,
        )
        .width(Length::Fill)
        .height(Length::Fixed(30.0))
        .style(move |theme: &theme::Theme, status| {
            let mut style = chip_style(on)(theme, status);
            style.border.radius = 6.0.into();
            style
        })
        .on_press(ViewMessage::Catalog(CatalogMessage::Activity(
            ActivityMessage::Blake2b(blake2b),
        )))
    };
    Row::new()
        .spacing(6)
        .push(half(false, "Bitcoin"))
        .push(half(true, "Bitcoin Blake2b"))
        .into()
}

/// What the menu says about the shown read: its block and source, or why
/// nothing shows.
fn activity_status(home: &Home) -> String {
    match home.catalog.activity.current(home.activity_chain()) {
        Some(Ok(snapshot)) => format!(
            "Block {} · {}",
            launcher_activity::block_label(snapshot.tip),
            snapshot.via.label()
        ),
        Some(Err(why)) => why.label().to_string(),
        None => "Reading network activity".to_string(),
    }
}

/// A dock menu's panel: 10px radius, a line border and a deep shadow.
fn menu_panel(theme: &theme::Theme) -> container::Style {
    container::Style {
        background: Some(iced::Background::Color(theme.colors.shell.panel)),
        border: Border {
            color: theme.colors.shell.line,
            width: 1.0,
            radius: 10.0.into(),
        },
        shadow: iced::Shadow {
            color: Color::from_rgba(0.0, 0.0, 0.0, 0.35),
            offset: iced::Vector::new(0.0, 18.0),
            blur_radius: 40.0,
        },
        ..Default::default()
    }
}

/// The activity source menu, above its dock button. It follows the Network
/// filter; picking a source saves the device preference.
pub(super) fn activity_layer<'a>(home: &Home) -> Option<Element<'a, ViewMessage>> {
    if !home.catalog.activity.menu || !home.activity_wanted() {
        return None;
    }
    let close = ViewMessage::Catalog(CatalogMessage::Activity(ActivityMessage::Menu(false)));
    let current = home.catalog.activity.source;
    let option = |source: NetworkActivitySource, label: &'static str| {
        let on = source == current;
        Action::new(
            activity_option_id(source),
            Container::new(
                Row::new()
                    .spacing(8)
                    .align_y(Alignment::Center)
                    .push(
                        Container::new(on.then(|| glyph(TICK, 14.0, Ink::Body)))
                            .width(Length::Fixed(14.0))
                            .height(Length::Fixed(14.0)),
                    )
                    .push(words(
                        label,
                        13.0,
                        if on { font::SEMIBOLD } else { font::REGULAR },
                        Ink::Body,
                    )),
            )
            .center_y(Length::Fill),
            focus,
        )
        .width(Length::Fill)
        .height(Length::Fixed(34.0))
        .padding([0.0, 10.0])
        .style(|theme: &theme::Theme, status| {
            let shell = theme.colors.shell;
            iced_button::Style {
                background: matches!(
                    status,
                    iced_button::Status::Hovered | iced_button::Status::Pressed
                )
                .then_some(iced::Background::Color(shell.selected)),
                text_color: shell.ink,
                border: Border {
                    radius: 6.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            }
        })
        .on_press(ViewMessage::ChangeDevicePreference(
            PreferenceChange::NetworkActivitySource(source),
        ))
    };
    let rule = Container::new(Space::new())
        .width(Length::Fill)
        .height(Length::Fixed(1.0))
        .style(|theme: &theme::Theme| container::Style {
            background: Some(iced::Background::Color(theme.colors.shell.line)),
            ..Default::default()
        });
    let networks = home
        .activity_blake2b_offered()
        .then(|| Container::new(activity_networks(home)).padding([4.0, 4.0]));
    let menu = Container::new(
        Column::new()
            .spacing(0)
            .push(
                Column::new()
                    .spacing(2)
                    .padding([6.0, 10.0])
                    .push(words(
                        localized_format!(
                            "Network activity · {}",
                            "Actividad de red · {}",
                            network_menu_name(home.activity_chain())
                        ),
                        12.0,
                        font::REGULAR,
                        Ink::Muted,
                    ))
                    .push(words(activity_status(home), 13.0, font::REGULAR, Ink::Body)),
            )
            .push(networks)
            .push(option(NetworkActivitySource::Automatic, "Automatic"))
            .push(option(NetworkActivitySource::LocalNode, "Local node"))
            .push(option(
                NetworkActivitySource::Connect,
                launcher_activity::public_source(home.activity_chain())
                    .map_or("COINCUBE | Connect", |(via, _)| via.label()),
            ))
            .push(
                Column::new()
                    .padding(Padding {
                        top: 6.0,
                        right: 6.0,
                        bottom: 4.0,
                        left: 6.0,
                    })
                    .spacing(8)
                    .push(rule)
                    .push(
                        text(localized_copy(
                            "Follows the Network filter, or the network picked above. Automatic \
                             reads a local node that is already running, else COINCUBE | Connect, \
                             or mempool.space for the test networks Connect does not serve. The \
                             Launcher never starts a node.",
                        ))
                        .size(12)
                        .style(|theme: &theme::Theme| text::Style {
                            color: Some(theme.colors.shell.muted),
                        })
                        .wrapping(text::Wrapping::WordOrGlyph),
                    )
                    .push(small_link(
                        Id::new("tenshu.launcher.activity.preferences"),
                        "Turn off in App preferences",
                        ViewMessage::OpenAppPreferences,
                    )),
            ),
    )
    .width(Length::Fixed(ACTIVITY_MENU_WIDTH))
    .padding(6)
    .style(menu_panel);
    // `bottom: calc(100% + 8px)` of its 32px button, like the Key card.
    let button_top = (BAR_HEIGHT + KEY_BUTTON_HEIGHT) / 2.0 + 8.0;
    let placed = Container::new(iced::widget::opaque(menu))
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(iced::alignment::Horizontal::Right)
        .align_y(iced::alignment::Vertical::Bottom)
        .padding(Padding {
            top: 0.0,
            right: GUTTER + key_footprint(home),
            bottom: button_top,
            left: 0.0,
        });
    Some(
        iced::widget::Stack::new()
            .width(Length::Fill)
            .height(Length::Fill)
            .push(mouse_area(Space::new().width(Length::Fill).height(Length::Fill)).on_press(close))
            .push(placed)
            .into(),
    )
}

/// The dock's width right of the Network activity button: the Key and its
/// gap, when the Key shows.
fn key_footprint(home: &Home) -> f32 {
    if launcher_catalog::project(home).filtered_total > 0 {
        KEY_BUTTON_WIDTH + 8.0
    } else {
        0.0
    }
}

pub(super) fn plan_id() -> Id {
    Id::new("tenshu.launcher.plan")
}

/// The plan's name, with a warning mark once the current network has no free
/// Cube slot. A click opens its per-network usage above it.
fn plan_button<'a>(home: &Home, projection: &Projection<'_>) -> Element<'a, ViewMessage> {
    let open = home.catalog.plan_menu;
    let full = projection.at_limit();
    Action::new(
        plan_id(),
        Container::new(
            Row::new()
                .spacing(8)
                .align_y(Alignment::Center)
                .push(full.then(|| glyph(ATTENTION, 16.0, Ink::Warn)))
                .push(words(
                    format!("{} plan", projection.tier),
                    13.0,
                    font::REGULAR,
                    Ink::Body,
                ))
                .push(glyph(CHEVRON, 14.0, Ink::Muted)),
        )
        .center_y(Length::Fill),
        focus,
    )
    .height(Length::Fixed(KEY_BUTTON_HEIGHT))
    .padding([0.0, 10.0])
    .style(menu_button_style(open))
    .on_press(ViewMessage::Catalog(CatalogMessage::PlanMenu(!open)))
    .into()
}

/// One network's line in the plan menu: its name, then the slots in use and
/// what the count leaves out or adds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PlanRow {
    pub(super) network: &'static str,
    pub(super) used: String,
    pub(super) note: Option<String>,
    pub(super) full: bool,
    /// The network Create and Import act on.
    pub(super) current: bool,
}

/// The plan's per-network limit and each network's use of it. Cubes that are
/// registered in Connect but not on this device are called out, because they
/// fill a slot without appearing in any list here.
pub(super) fn plan_rows(projection: &Projection<'_>) -> Vec<PlanRow> {
    let limit = projection.cube_limit;
    projection
        .usage
        .iter()
        .map(|usage| {
            let network = network_name(usage.chain);
            let Some(total) = usage.total() else {
                // This device's share is unknown: say what Connect holds
                // rather than print a ratio that may be short.
                return PlanRow {
                    network,
                    used: "Not counted".to_string(),
                    note: Some(if usage.remote > 0 {
                        localized_format!(
                            "{} in Connect; this device not counted",
                            "{} en Connect; este dispositivo no cuenta",
                            usage.remote
                        )
                    } else {
                        "This device's Cubes could not be counted".to_string()
                    }),
                    full: false,
                    current: usage.chain == projection.network,
                };
            };
            PlanRow {
                network,
                used: if total > limit {
                    format!("{total} · plan includes {limit}")
                } else {
                    format!("{total} of {limit}")
                },
                note: (usage.remote > 0).then(|| format!("{} only in Connect", usage.remote)),
                full: total >= limit,
                current: usage.chain == projection.network,
            }
        })
        .collect()
}

/// Why Create and Import are off, when the current network is full.
fn plan_limit_note(projection: &Projection<'_>) -> Option<String> {
    projection.at_limit().then(|| {
        if projection.total_count > projection.cube_limit {
            format!(
                "Existing Cubes keep working. To add one, upgrade, or delete {} Cubes until \
                 you're under the plan's limit.",
                network_name(projection.network),
            )
        } else {
            format!(
                "Upgrade, or delete a {} Cube, to add one.",
                network_name(projection.network),
            )
        }
    })
}

/// The plan menu, above its dock button: the limit, each network's use of
/// it, and the way to more slots.
pub(super) fn plan_layer<'a>(
    home: &Home,
    projection: &Projection<'_>,
) -> Option<Element<'a, ViewMessage>> {
    if !home.catalog.plan_menu {
        return None;
    }
    let close = ViewMessage::Catalog(CatalogMessage::PlanMenu(false));
    let mut rows = Column::new().spacing(0);
    for row in plan_rows(projection) {
        let mut title = Row::new().spacing(6).align_y(Alignment::Center).push(words(
            row.network,
            13.0,
            font::REGULAR,
            Ink::Body,
        ));
        if row.current {
            // Create and Import add Cubes here; the dock no longer captions it.
            title = title.push(words(
                localized_copy("New Cubes"),
                12.0,
                font::MEDIUM,
                Ink::Accent,
            ));
        }
        let mut name = Column::new().spacing(2).width(Length::Fill).push(title);
        if let Some(note) = row.note {
            name = name.push(words(note, 12.0, font::REGULAR, Ink::Muted));
        }
        let mut used = Row::new().spacing(6).align_y(Alignment::Center).push(words(
            row.used,
            13.0,
            font::MEDIUM,
            Ink::Body,
        ));
        if row.full {
            used = used.push(words(localized_copy("Full"), 12.0, font::MEDIUM, Ink::Warn));
        }
        rows = rows.push(
            Container::new(
                Row::new()
                    .spacing(12)
                    .align_y(Alignment::Center)
                    .push(name)
                    .push(used),
            )
            .padding([6.0, 10.0]),
        );
    }
    let rule = Container::new(Space::new())
        .width(Length::Fill)
        .height(Length::Fixed(1.0))
        .style(|theme: &theme::Theme| container::Style {
            background: Some(iced::Background::Color(theme.colors.shell.line)),
            ..Default::default()
        });
    // The note and the link label start where the rows' text does, 10px in:
    // the foot sits 4px in so the link's own 6px padding lands its label
    // there, and the rule and note take the remaining inset themselves.
    let mut foot = Column::new()
        .padding(Padding {
            top: 6.0,
            right: 4.0,
            bottom: 4.0,
            left: 4.0,
        })
        .spacing(8)
        .push(Container::new(rule).padding([0.0, 2.0]));
    if let Some(note) = plan_limit_note(projection) {
        foot = foot.push(
            Container::new(
                text(note)
                    .size(12)
                    .style(|theme: &theme::Theme| text::Style {
                        color: Some(theme.colors.shell.warn),
                    })
                    .wrapping(text::Wrapping::WordOrGlyph),
            )
            .padding([0.0, 6.0]),
        );
    }
    let foot = foot.push(if home.connect_account.is_authenticated() {
        small_link(
            Id::new("tenshu.launcher.plan.billing"),
            "Plan & Billing",
            ViewMessage::GoToSection(HomeSection::Connect(
                crate::app::menu::ConnectSubMenu::PlanBilling,
            )),
        )
    } else {
        small_link(
            Id::new("tenshu.launcher.plan.sign-in"),
            "Sign in to Connect to change plans",
            ViewMessage::GoToSection(HomeSection::Connect(
                crate::app::menu::ConnectSubMenu::Overview,
            )),
        )
    });
    let menu = Container::new(
        Column::new()
            .spacing(0)
            .push(
                Container::new(words(
                    localized_format!(
                        "{} plan · {} Cubes per network",
                        "Plan {} · {} Cubes por red",
                        projection.tier,
                        projection.cube_limit
                    ),
                    12.0,
                    font::REGULAR,
                    Ink::Muted,
                ))
                .padding([6.0, 10.0]),
            )
            .push(rows)
            .push(foot),
    )
    .width(Length::Fixed(PLAN_MENU_WIDTH))
    .padding(6)
    .style(menu_panel);
    // Over its own button: right of it sit Network activity and the Key.
    let activity = if home.activity_wanted() {
        ACTIVITY_BUTTON_WIDTH + 8.0
    } else {
        0.0
    };
    let button_top = (BAR_HEIGHT + KEY_BUTTON_HEIGHT) / 2.0 + 8.0;
    let placed = Container::new(iced::widget::opaque(menu))
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(iced::alignment::Horizontal::Right)
        .align_y(iced::alignment::Vertical::Bottom)
        .padding(Padding {
            top: 0.0,
            right: GUTTER + key_footprint(home) + activity,
            bottom: button_top,
            left: 0.0,
        });
    Some(
        iced::widget::Stack::new()
            .width(Length::Fill)
            .height(Length::Fill)
            .push(mouse_area(Space::new().width(Length::Fill).height(Length::Fill)).on_press(close))
            .push(placed)
            .into(),
    )
}

/// `.keybtn`: hover peeks, a click pins, a second click or Escape shuts.
fn key_button<'a>(open: bool) -> Element<'a, ViewMessage> {
    let button = Action::new(
        launcher_catalog::key_id(),
        Container::new(
            Row::new()
                .spacing(6)
                .align_y(Alignment::Center)
                .push(glyph(KEY, 16.0, Ink::Body))
                .push(words(localized_copy("Key"), 13.0, font::REGULAR, Ink::Body)),
        )
        .center_y(Length::Fill),
        focus,
    )
    .width(Length::Fixed(KEY_BUTTON_WIDTH))
    .height(Length::Fixed(KEY_BUTTON_HEIGHT))
    .padding([0.0, 10.0])
    .style(menu_button_style(open))
    .on_press(ViewMessage::Catalog(CatalogMessage::KeyToggle));
    mouse_area(button)
        .on_enter(ViewMessage::Catalog(CatalogMessage::KeyHover(
            KeyPart::Button,
            true,
        )))
        .on_exit(ViewMessage::Catalog(CatalogMessage::KeyHover(
            KeyPart::Button,
            false,
        )))
        .into()
}

fn coin<'a>(chain: ChainId, light: bool, greyed: bool) -> Element<'a, ViewMessage> {
    let source = if greyed {
        // As a ghost's top face: `filter: grayscale(1); opacity: .55`.
        let ink = if light { "#30302d" } else { "#ecebe6" };
        format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 16 16\" color=\"{ink}\"><filter id=\"g\"><feColorMatrix type=\"saturate\" values=\"0\"/></filter><g filter=\"url(#g)\" opacity=\".55\">{}</g></svg>",
            launcher_coin::fragment(chain, 0.0, 0.0, 16.0, 1.5, 16.0 * 0.018)
        )
    } else {
        launcher_coin::widget_source(chain, 16.0, 1.5, light)
    };
    svg(svg::Handle::from_memory(source.into_bytes()))
        .width(Length::Fixed(16.0))
        .height(Length::Fixed(16.0))
        .into()
}

/// `.sw`: a 12px face swatch in a network family's edge colour.
fn swatch<'a>(chain: ChainId, light: bool) -> Element<'a, ViewMessage> {
    let edge = launcher_scene::edge(chain, light);
    let face = launcher_scene::Palette::new(light).top;
    Container::new(Space::new())
        .width(Length::Fixed(12.0))
        .height(Length::Fixed(12.0))
        .style(move |_: &theme::Theme| container::Style {
            background: Some(iced::Background::Color(Color::from_rgb8(
                face[0], face[1], face[2],
            ))),
            border: Border {
                color: Color::from_rgba8(edge[0], edge[1], edge[2], edge[3] as f32 / 255.0),
                width: 1.5,
                radius: 2.0.into(),
            },
            ..Default::default()
        })
        .into()
}

fn rgba([r, g, b, a]: [u8; 4]) -> String {
    format!("rgba({r},{g},{b},{:.3})", a as f32 / 255.0)
}

/// The Key's column heads: a 20px Cube, solid for one on this device,
/// see-through with its far edges dashed for one only in Connect.
fn mini_cube<'a>(ghost: bool, light: bool) -> Element<'a, ViewMessage> {
    let (edge, fill, back) = if ghost {
        (
            rgba(launcher_scene::ghost_edge(ChainId::Bitcoin, light)),
            rgba(launcher_scene::ghost_fill(
                launcher_scene::Ghost::Restorable,
                light,
            )),
            "<path d=\"M12 3v9M4 16.5l8-4.5 8 4.5\" stroke-dasharray=\"1.5 1.5\" opacity=\".7\"/>",
        )
    } else {
        let [r, g, b] = launcher_scene::Palette::new(light).top;
        (
            rgba(launcher_scene::edge(ChainId::Bitcoin, light)),
            format!("rgb({r},{g},{b})"),
            "",
        )
    };
    let source = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 24 24\" fill=\"none\" stroke=\"{edge}\" stroke-width=\"1.5\" stroke-linejoin=\"round\"><path d=\"M12 3l8 4.5v9L12 21l-8-4.5v-9z\" fill=\"{fill}\"/><path d=\"M4 7.5l8 4.5 8-4.5M12 12v9\"/>{back}</svg>"
    );
    svg(svg::Handle::from_memory(source.into_bytes()))
        .width(Length::Fixed(20.0))
        .height(Length::Fixed(20.0))
        .into()
}

/// One line of a Key cell: its marks, then what they mean.
fn mark<'a>(
    marks: Vec<Element<'a, ViewMessage>>,
    label: &'static str,
    tone: Ink,
) -> Element<'a, ViewMessage> {
    let mut row = Row::new().spacing(6).align_y(Alignment::Center);
    for mark in marks {
        row = row.push(mark);
    }
    if label.is_empty() {
        row.into()
    } else {
        row.push(Container::new(words(label, 13.0, font::REGULAR, tone)).width(Length::Fill))
            .into()
    }
}

fn rule_line<'a>() -> Element<'a, ViewMessage> {
    Container::new(Space::new())
        .width(Length::Fill)
        .height(Length::Fixed(1.0))
        .style(|theme: &theme::Theme| container::Style {
            background: Some(iced::Background::Color(theme.colors.shell.line)),
            ..Default::default()
        })
        .into()
}

/// A Key row: the face and what it carries, then one cell per column,
/// under a rule unless it opens the card.
fn face_row<'a>(
    ruled: bool,
    face: &'static str,
    carries: &'static str,
    cells: Vec<Column<'a, ViewMessage>>,
) -> Element<'a, ViewMessage> {
    let mut row = Row::new().spacing(14).push(
        Column::new()
            .width(Length::Fixed(KEY_FACE_WIDTH))
            .push(words(face, 13.0, font::MEDIUM, Ink::Body))
            .push(words(carries, 13.0, font::REGULAR, Ink::Muted)),
    );
    for cell in cells {
        row = row.push(cell.spacing(6).width(Length::FillPortion(1)));
    }
    let row = Container::new(row).padding([9.0, 0.0]);
    if ruled {
        Column::new().push(rule_line()).push(row).into()
    } else {
        // The card's first row: nothing above it to rule off.
        row.padding(Padding {
            bottom: 9.0,
            ..Padding::ZERO
        })
        .into()
    }
}

/// The networks the Key explains: Blake2b and the test networks only when
/// the user has them shown (Developer Mode, the Blake2b release).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct KeyNetworks {
    blake2b: bool,
    test: bool,
}

impl KeyNetworks {
    fn of(displayed: &[ChainId]) -> Self {
        Self {
            blake2b: displayed
                .iter()
                .any(|c| c.is_blake2b() && !launcher_coin::is_test_network(*c)),
            test: displayed.iter().any(|c| launcher_coin::is_test_network(*c)),
        }
    }

    fn coins(self) -> Vec<ChainId> {
        let mut coins = vec![ChainId::Bitcoin];
        if self.blake2b {
            coins.push(ChainId::BitcoinBlake2b);
        }
        coins
    }
}

/// `.keycard`: how to read a Cube, one row per face. A second column shows
/// the marks a Cube only in Connect draws instead, when the floor stands
/// any (G-Key, Robert 2026-11-07).
pub(super) fn key_card<'a>(home: &Home, ghosts: bool) -> Element<'a, ViewMessage> {
    let light = matches!(
        home.theme_mode,
        coincube_ui::theme::palette::ThemeMode::Light
    );
    let networks = KeyNetworks::of(&home.displayed_networks);
    let coins = |greyed| {
        networks
            .coins()
            .into_iter()
            .map(|chain| coin(chain, light, greyed))
            .collect::<Vec<_>>()
    };
    let cells = |local: Column<'a, ViewMessage>, connect: Column<'a, ViewMessage>| {
        if ghosts {
            vec![local, connect]
        } else {
            vec![local]
        }
    };

    let head = |label: &'static str, ghost: bool| {
        Row::new()
            .spacing(6)
            .align_y(Alignment::Center)
            .width(Length::FillPortion(1))
            .push(mini_cube(ghost, light))
            .push(words(label, 11.0, font::REGULAR, Ink::Muted))
    };
    let heads = Row::new()
        .spacing(14)
        .padding(Padding {
            bottom: 8.0,
            ..Padding::ZERO
        })
        .push(
            Container::new(words(
                localized_copy("Face"),
                11.0,
                font::REGULAR,
                Ink::Muted,
            ))
            .width(Length::Fixed(KEY_FACE_WIDTH)),
        )
        .push(head("On this device", false))
        .push(head("Only in Connect", true));

    let top = face_row(
        ghosts,
        "Top",
        "network",
        cells(
            Column::new().push(mark(coins(false), "Network coin", Ink::Body)),
            Column::new().push(mark(coins(true), "Same coin, greyed", Ink::Body)),
        ),
    );
    let left = face_row(
        true,
        "Left",
        "wallets",
        cells(
            Column::new()
                .push(mark(
                    vec![
                        glyph(SHIELD, 16.0, Ink::Body),
                        glyph(WALLET, 16.0, Ink::Body),
                        glyph(SPARK, 16.0, Ink::Body),
                    ],
                    "",
                    Ink::Body,
                ))
                .push(words(
                    localized_copy("Vault, imported wallet, Spark"),
                    13.0,
                    font::REGULAR,
                    Ink::Muted,
                )),
            Column::new()
                .push(mark(
                    vec![
                        glyph(SHIELD, 16.0, Ink::Muted),
                        glyph(SPARK, 16.0, Ink::Muted),
                        glyph(CLOUD, 16.0, Ink::Body),
                    ],
                    "",
                    Ink::Body,
                ))
                .push(words(
                    localized_copy("Wallets Connect knows of, greyed, then a cloud"),
                    13.0,
                    font::REGULAR,
                    Ink::Muted,
                )),
        ),
    );
    let right = face_row(
        true,
        "Right",
        if ghosts { "lock or kit" } else { "lock" },
        cells(
            Column::new()
                .push(mark(
                    vec![
                        glyph(LOCK, 16.0, Ink::Body),
                        glyph(PASSKEY, 16.0, Ink::Body),
                    ],
                    "PIN or passkey",
                    Ink::Body,
                ))
                .push(mark(
                    vec![glyph(ATTENTION, 16.0, Ink::Warn)],
                    "Backup not confirmed",
                    Ink::Body,
                ))
                .push(mark(
                    vec![glyph(QUESTION, 16.0, Ink::Muted)],
                    "Backup status unknown",
                    Ink::Body,
                )),
            Column::new()
                .push(mark(
                    vec![glyph(KIT, 16.0, Ink::Good)],
                    "Recovery Kit: can restore here",
                    Ink::Body,
                ))
                .push(mark(
                    vec![
                        glyph(NOKIT, 16.0, Ink::Muted),
                        glyph(ATTENTION, 16.0, Ink::Warn),
                    ],
                    "No Recovery Kit",
                    Ink::Body,
                )),
        ),
    );

    let mut edges = Row::new()
        .spacing(14)
        .align_y(Alignment::Center)
        .push(words("Edge", 13.0, font::REGULAR, Ink::Muted));
    let mut families = vec![(ChainId::Bitcoin, "Bitcoin")];
    if networks.blake2b {
        families.push((ChainId::BitcoinBlake2b, "Blake2b"));
    }
    if networks.test {
        families.push((ChainId::Testnet4, "Test networks"));
    }
    for (chain, name) in families {
        edges = edges.push(
            Row::new()
                .spacing(6)
                .align_y(Alignment::Center)
                .push(swatch(chain, light))
                .push(words(name, 13.0, font::REGULAR, Ink::Body)),
        );
    }

    let mut card = Column::new();
    if ghosts {
        card = card.push(heads);
    }
    card = card.push(top).push(left).push(right);
    if ghosts {
        card = card.push(face_row(
            true,
            "Body",
            "fill",
            vec![
                Column::new().push(mark(vec![], "Solid", Ink::Body)),
                Column::new()
                    .push(mark(vec![], "See-through", Ink::Body))
                    .push(words(
                        "Faint fill only when it has a Recovery Kit",
                        13.0,
                        font::REGULAR,
                        Ink::Muted,
                    )),
            ],
        ));
    }
    card = card
        .push(rule_line())
        .push(Container::new(edges).padding(Padding {
            top: 10.0,
            ..Padding::ZERO
        }));
    if ghosts {
        card = card.push(
            Container::new(words(
                localized_copy(
                    "Select a Cube that's only in Connect to restore it or delete it from Connect.",
                ),
                12.0,
                font::REGULAR,
                Ink::Muted,
            ))
            .padding(Padding {
                top: 10.0,
                ..Padding::ZERO
            }),
        );
    }

    Container::new(card)
        .width(Length::Fixed(if ghosts {
            KEY_CARD_WIDE
        } else {
            KEY_CARD_WIDTH
        }))
        .padding(Padding {
            top: 14.0,
            right: 16.0,
            bottom: 14.0,
            left: 16.0,
        })
        .style(|theme: &theme::Theme| container::Style {
            background: Some(iced::Background::Color(theme.colors.shell.panel)),
            border: Border {
                color: theme.colors.shell.line,
                width: 1.0,
                radius: 8.0.into(),
            },
            ..Default::default()
        })
        .into()
}

/// An undrawn copy of the bar's brand (`top_bar::brand_mark`), to place a
/// menu under a control after it. Iced's SVG tint drops alpha, so the cube's
/// place is a space.
fn brand_footprint<'a, M: 'a>() -> Element<'a, M> {
    Row::new()
        .spacing(8)
        .align_y(Alignment::Center)
        .push(
            Space::new()
                .width(Length::Fixed(20.0))
                .height(Length::Fixed(20.0)),
        )
        .push(words("Tenshu", 15.0, font::SEMIBOLD, Ink::Clear))
        .into()
}

/// The Network menu's width: wide enough for "Bitcoin Blake2b mainnet" beside
/// a "7 + 7 in Connect" count on one line.
const NETWORK_MENU_WIDTH: f32 = 320.0;

/// `.menu`: 320px under the chip, one `.mi` row per listed network and the
/// filter note. Each row counts the network's Cubes against its plan limit:
/// the ones on this device, plus "+ N in Connect" for Cubes registered in
/// Connect but not here, which use a slot too. A click outside it closes it.
pub(super) fn network_layer<'a>(
    home: &Home,
    projection: &Projection<'_>,
) -> Option<Element<'a, ViewMessage>> {
    if !home.catalog.network_menu {
        return None;
    }
    let close = ViewMessage::Catalog(CatalogMessage::NetworkMenu(false));
    let mut rows = Column::new().spacing(0);
    for (chain, count) in &projection.network_counts {
        let chain = *chain;
        let on = projection.filter == NetworkFilter::Network(chain);
        let row = Row::new()
            .spacing(8)
            .align_y(Alignment::Center)
            .push(
                Container::new(on.then(|| glyph(TICK, 14.0, Ink::Body)))
                    .width(Length::Fixed(14.0))
                    .height(Length::Fixed(14.0)),
            )
            .push(words(
                network_menu_name(chain),
                13.0,
                if on { font::SEMIBOLD } else { font::REGULAR },
                Ink::Body,
            ))
            .push(Space::new().width(Length::Fill))
            .push(words(
                network_menu_count(projection, chain, *count),
                12.0,
                font::REGULAR,
                Ink::Muted,
            ));
        rows = rows.push(
            Action::new(
                network_option_id(chain),
                Container::new(row).center_y(Length::Fill),
                focus,
            )
            .width(Length::Fill)
            .height(Length::Fixed(34.0))
            .padding([0.0, 10.0])
            .style(|theme: &theme::Theme, status| {
                let shell = theme.colors.shell;
                iced_button::Style {
                    background: matches!(
                        status,
                        iced_button::Status::Hovered | iced_button::Status::Pressed
                    )
                    .then_some(iced::Background::Color(shell.selected)),
                    text_color: shell.ink,
                    border: Border {
                        radius: 6.0.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                }
            })
            // Retry is possible while the filtered network is not yet current.
            .on_press(if on && chain == home.network {
                close.clone()
            } else {
                ViewMessage::Catalog(CatalogMessage::Filter(NetworkFilter::Network(chain)))
            }),
        );
    }
    let note = Column::new()
        .padding(Padding {
            top: 6.0,
            right: 6.0,
            bottom: 4.0,
            left: 6.0,
        })
        .push(
            Container::new(Space::new())
                .width(Length::Fill)
                .height(Length::Fixed(1.0))
                .style(|theme: &theme::Theme| container::Style {
                    background: Some(iced::Background::Color(theme.colors.shell.line)),
                    ..Default::default()
                }),
        )
        .push(Space::new().height(Length::Fixed(8.0)))
        .push(words(
            localized_copy("Filters change what is shown, never a Cube's network."),
            12.0,
            font::REGULAR,
            Ink::Muted,
        ));
    let menu = Container::new(rows.push(note))
        .width(Length::Fixed(NETWORK_MENU_WIDTH))
        .padding(6)
        .style(|theme: &theme::Theme| container::Style {
            background: Some(iced::Background::Color(theme.colors.shell.panel)),
            border: Border {
                color: theme.colors.shell.line,
                width: 1.0,
                radius: 10.0.into(),
            },
            // `0 18px 40px -12px rgba(0,0,0,.5)`; Iced has no spread.
            shadow: iced::Shadow {
                color: Color::from_rgba(0.0, 0.0, 0.0, 0.35),
                offset: iced::Vector::new(0.0, 18.0),
                blur_radius: 40.0,
            },
            ..Default::default()
        });
    // `.menu { top: 40px }` from the chip, which sits 16px down the bar. An
    // undrawn copy of the brand and separator puts it under the chip.
    let placed = Column::new()
        .push(Space::new().height(Length::Fixed((BAR_HEIGHT - 32.0) / 2.0 + 40.0)))
        .push(
            Row::new()
                .spacing(16)
                .push(brand_footprint())
                .push(Space::new().width(Length::Fixed(1.0)))
                // Presses on the menu's own padding and note stay in it.
                .push(under_network_chip(home, iced::widget::opaque(menu))),
        )
        .padding([0.0, GUTTER]);
    Some(
        iced::widget::Stack::new()
            .width(Length::Fill)
            .height(Length::Fill)
            .push(mouse_area(Space::new().width(Length::Fill).height(Length::Fill)).on_press(close))
            .push(placed)
            .into(),
    )
}

/// The Key card's layer: bottom-right, 8px above the dock.
pub(super) fn key_layer<'a>(
    home: &Home,
    projection: &Projection<'_>,
) -> Option<Element<'a, ViewMessage>> {
    let ghosts = projection.floor_eligible(home.catalog.mode) && !projection.ghosts.is_empty();
    // A hover peek waits while a dock menu is open: one popover at a time.
    let menu = home.catalog.plan_menu || home.catalog.activity.menu;
    (home.catalog.key_open() && !menu).then(|| {
        // `bottom: calc(100% + 8px)` of the 32px button centred in the dock.
        // The 8px gap stays inside the hover area so a peek survives the
        // pointer's way up from the button to the card.
        let button_top = (BAR_HEIGHT + KEY_BUTTON_HEIGHT) / 2.0;
        Container::new(
            mouse_area(Container::new(key_card(home, ghosts)).padding(Padding {
                bottom: 8.0,
                ..Padding::ZERO
            }))
            .on_enter(ViewMessage::Catalog(CatalogMessage::KeyHover(
                KeyPart::Card,
                true,
            )))
            .on_exit(ViewMessage::Catalog(CatalogMessage::KeyHover(
                KeyPart::Card,
                false,
            ))),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(iced::alignment::Horizontal::Right)
        .align_y(iced::alignment::Vertical::Bottom)
        .padding(Padding {
            top: 0.0,
            right: GUTTER,
            bottom: button_top,
            left: 0.0,
        })
        .into()
    })
}

/// What the PIN and passkey screens need to redraw the Launcher's bars
/// exactly as they stood when the Cube was opened from the floor.
#[derive(Debug, Clone, Default)]
pub(crate) struct ChromeSnapshot {
    /// The filter chips' labels, the Network chip last.
    chips: Vec<String>,
    /// Whether "Clear filters" shows.
    filtered: bool,
    count: String,
    connect: Vec<(String, Ink, f32, bool)>,
    links: Vec<&'static str>,
    /// The dock's plan button: its label, and whether the current network is
    /// full.
    pub(super) plan: (String, bool),
    activity: bool,
    key: bool,
}

pub(super) fn snapshot(home: &Home, projection: &Projection<'_>) -> ChromeSnapshot {
    use crate::app::view::shared::connect_control::ConnectDetail;
    let account = &home.connect_account;
    let mut connect = vec![("Connect".to_string(), Ink::Muted, 13.0, false)];
    connect.push(("·".to_string(), Ink::Muted, 13.0, false));
    connect.push(match launcher_connect_state(home).0 {
        ConnectDetail::Account(email) => (email.to_string(), Ink::Muted, 13.0, false),
        ConnectDetail::Offline => ("Offline".to_string(), Ink::Muted, 13.0, false),
        ConnectDetail::Checking => ("Checking…".to_string(), Ink::Muted, 13.0, false),
        ConnectDetail::SignIn => ("Sign in".to_string(), Ink::Accent, 13.0, true),
    });
    let mut links = Vec::new();
    if account.is_authenticated()
        && !home.network.is_blake2b()
        && feature_flags::RECOVER_VAULT_ENABLED
    {
        links.push("Recover a Vault");
    }
    ChromeSnapshot {
        chips: group_chips(home)
            .into_iter()
            .map(|(_, label, _)| label.to_string())
            .chain(std::iter::once(match projection.filter {
                NetworkFilter::Network(chain) => format!("Network: {}", network_menu_name(chain)),
                _ => "Network".to_string(),
            }))
            .collect(),
        filtered: projection.filter != NetworkFilter::All,
        count: count_text(
            projection.locals.len(),
            projection.local_total,
            projection.connect_only(home.catalog.mode),
        ),
        connect,
        links,
        plan: (format!("{} plan", projection.tier), projection.at_limit()),
        activity: home.activity_wanted(),
        key: projection.filtered_total > 0,
    }
}

/// P-Launcher's `countText`: this device's Cubes, then the ghosts beside
/// them, which are Connect's and never counted as this device's.
fn count_text(shown: usize, total: usize, in_connect: usize) -> String {
    let connect = if in_connect > 0 {
        format!(" · {in_connect} in Connect only")
    } else {
        String::new()
    };
    if total == 0 && in_connect > 0 {
        format!("No Cubes on this device{connect}")
    } else if shown == total {
        format!(
            "{total} {}{connect}",
            if total == 1 { "Cube" } else { "Cubes" }
        )
    } else {
        format!(
            "{shown} of {total} Cubes{connect} · {} hidden",
            total - shown
        )
    }
}

/// A control drawn in its resting state, for bars that take no input.
fn resting<'a, M: 'a>(
    content: impl Into<Element<'a, M>>,
    height: f32,
    padding: f32,
    style: impl Fn(&theme::Theme) -> iced_button::Style + 'a,
) -> Element<'a, M> {
    Container::new(Container::new(content).center_y(Length::Fill))
        .height(Length::Fixed(height))
        .padding([0.0, padding])
        .style(move |theme: &theme::Theme| {
            let button = style(theme);
            container::Style {
                background: button.background,
                border: button.border,
                text_color: Some(button.text_color),
                ..Default::default()
            }
        })
        .into()
}

/// The `.top.quiet` bar under a Cube that is being unlocked.
pub(crate) fn quiet_top_bar<'a, M: 'a>(chrome: &ChromeSnapshot) -> Element<'a, M> {
    let quiet = true;
    // `.quiet .chip.on` drops the fill and the weight, so every chip rests.
    let last = chrome.chips.len().saturating_sub(1);
    let mut chips = Row::new().spacing(8).align_y(Alignment::Center);
    for (index, label) in chrome.chips.iter().enumerate() {
        let mut content = Row::new().spacing(6).align_y(Alignment::Center).push(words(
            label.clone(),
            13.0,
            font::REGULAR,
            body(quiet),
        ));
        if index == last {
            content = content.push(glyph(CHEVRON, 14.0, body(quiet)));
        }
        chips = chips.push(resting(content, 32.0, 12.0, |theme| {
            chip_style(false)(theme, iced_button::Status::Active)
        }));
    }
    let mut connect =
        Row::new()
            .spacing(8)
            .align_y(Alignment::Center)
            .push(glyph(PERSON, 16.0, body(quiet)));
    for (value, tone, size, strong) in &chrome.connect {
        connect = connect.push(words(
            value.clone(),
            *size,
            if *strong {
                font::SEMIBOLD
            } else {
                font::REGULAR
            },
            *tone,
        ));
    }
    let resting_bar = |theme: &theme::Theme| bar_button(theme, iced_button::Status::Active, false);
    top_bar::top_bar(
        top_bar::brand_mark(quiet),
        top_bar::resting_icon(SLIDERS, APP_PREFERENCES),
    )
    .lead(separator())
    .lead(chips)
    .lead(words(chrome.count.clone(), 13.0, font::REGULAR, Ink::Muted))
    .lead(chrome.filtered.then(|| {
        Container::new(words(
            localized_copy("Clear filters"),
            13.0,
            font::REGULAR,
            Ink::Muted,
        ))
        .padding([2.0, 4.0])
    }))
    .chip(resting(connect, 32.0, 10.0, resting_bar))
    .into()
}

/// The `.bottom.quiet` dock under a Cube that is being unlocked.
pub(crate) fn quiet_dock<'a, M: 'a>(chrome: &ChromeSnapshot) -> Element<'a, M> {
    let quiet = true;
    let action = |icon: &'static str, label: &'static str| {
        resting(
            Row::new()
                .spacing(8)
                .align_y(Alignment::Center)
                .push(glyph(icon, 16.0, body(quiet)))
                .push(words(label, 14.0, font::MEDIUM, body(quiet))),
            36.0,
            16.0,
            |theme| dock_button(theme, iced_button::Status::Active, false),
        )
    };
    let mut row = Row::new()
        .spacing(8)
        .align_y(Alignment::Center)
        .height(Length::Fill)
        .push(action(PLUS, coincube_ui::locale::copy("Create Cube")))
        .push(action(
            IMPORT,
            coincube_ui::locale::copy("Import or restore Bitcoin wallet"),
        ));
    for link in &chrome.links {
        row = row.push(
            Container::new(words(*link, 13.0, font::MEDIUM, Ink::Accent)).padding([4.0, 6.0]),
        );
    }
    row = row.push(Space::new().width(Length::Fill));
    let resting_bar = |theme: &theme::Theme| bar_button(theme, iced_button::Status::Active, false);
    let (plan, full) = &chrome.plan;
    row = row.push(resting(
        Row::new()
            .spacing(8)
            .align_y(Alignment::Center)
            .push(full.then(|| glyph(ATTENTION, 16.0, Ink::Warn)))
            .push(words(plan.clone(), 13.0, font::REGULAR, body(quiet)))
            .push(glyph(CHEVRON, 14.0, Ink::Muted)),
        KEY_BUTTON_HEIGHT,
        10.0,
        resting_bar,
    ));
    if chrome.activity {
        row = row.push(
            Container::new(resting(
                Row::new()
                    .spacing(8)
                    .align_y(Alignment::Center)
                    .push(words(
                        localized_copy("Network activity"),
                        13.0,
                        font::REGULAR,
                        body(quiet),
                    ))
                    .push(Space::new().width(Length::Fill))
                    .push(glyph(CHEVRON, 14.0, Ink::Muted)),
                KEY_BUTTON_HEIGHT,
                10.0,
                resting_bar,
            ))
            .width(Length::Fixed(ACTIVITY_BUTTON_WIDTH)),
        );
    }
    // `.keybtn` is outside `.quiet`'s selectors, so it keeps its ink.
    let key = chrome.key.then(|| {
        resting(
            Row::new()
                .spacing(6)
                .align_y(Alignment::Center)
                .push(glyph(KEY, 16.0, Ink::Body))
                .push(words(localized_copy("Key"), 13.0, font::REGULAR, Ink::Body)),
            KEY_BUTTON_HEIGHT,
            10.0,
            |theme| bar_button(theme, iced_button::Status::Active, false),
        )
    });
    Container::new(row.push(key))
        .height(Length::Fixed(BAR_HEIGHT))
        .padding([0.0, GUTTER])
        .width(Length::Fill)
        .style(rail)
        .into()
}

#[cfg(test)]
mod tests {
    use super::{count_text, KeyNetworks};
    use crate::chain::ChainId;

    #[test]
    fn the_key_explains_only_the_networks_the_user_has_shown() {
        let only = |displayed: &[ChainId]| KeyNetworks::of(displayed);
        assert_eq!(
            only(&[ChainId::Bitcoin]),
            KeyNetworks {
                blake2b: false,
                test: false
            }
        );
        assert_eq!(
            only(&[ChainId::Bitcoin, ChainId::BitcoinBlake2b]),
            KeyNetworks {
                blake2b: true,
                test: false
            }
        );
        // Developer Mode without the Blake2b release: test networks only.
        assert_eq!(
            only(&[ChainId::Bitcoin, ChainId::Testnet4, ChainId::Signet]),
            KeyNetworks {
                blake2b: false,
                test: true
            }
        );
        // Blake2b testnet alone is a test network, not the Blake2b family.
        assert_eq!(
            only(&[ChainId::Bitcoin, ChainId::BitcoinBlake2bTestnet4]),
            KeyNetworks {
                blake2b: false,
                test: true
            }
        );
        assert_eq!(
            only(&[ChainId::Bitcoin, ChainId::BitcoinBlake2b]).coins(),
            vec![ChainId::Bitcoin, ChainId::BitcoinBlake2b]
        );
    }

    #[test]
    fn the_count_names_this_devices_cubes_and_the_ghosts_apart() {
        assert_eq!(count_text(2, 2, 0), "2 Cubes");
        assert_eq!(count_text(1, 1, 2), "1 Cube · 2 in Connect only");
        assert_eq!(
            count_text(0, 0, 2),
            "No Cubes on this device · 2 in Connect only"
        );
        assert_eq!(
            count_text(1, 3, 1),
            "1 of 3 Cubes · 1 in Connect only · 2 hidden"
        );
    }
}

/// The Launcher's bars on the shared top bar. The capture test writes frames
/// beside the Cube's (`app::view::nav::shell::capture_tests`), opt-in only:
/// `TOP_BAR_CAPTURE_DIR=<dir> cargo test -p coincube-gui --lib top_bar_capture
/// -- --ignored`. Every Cube is a synthetic fixture.
#[cfg(test)]
mod top_bar_tests {
    use std::path::PathBuf;

    use iced::{
        advanced::{
            layout,
            renderer::Headless,
            widget::{operation::Focusable, Id, Operation, Tree},
            Layout,
        },
        widget::rule,
        Rectangle, Size,
    };

    use super::super::{launcher_catalog, Home, State};
    use crate::app::state::connect::ConnectFlowStep;
    use crate::app::view::nav::shell::capture_tests::{directory, renderer, save, themes, WINDOWS};
    use crate::{app::settings::CubeSettings, chain::ChainId, dir::CoincubeDirectory};

    fn home(light: bool) -> Home {
        let mut home = Home::new(
            CoincubeDirectory::new(PathBuf::new()),
            Some(coincube_core::miniscript::bitcoin::Network::Bitcoin),
        )
        .0;
        home.network = ChainId::Bitcoin;
        let cubes = ["Everyday", "Archive", "Household 2024", "Travel fund"]
            .iter()
            .enumerate()
            .map(|(i, name)| {
                CubeSettings::new_with_raw_id(format!("cube-{i}"), (*name).into(), ChainId::Bitcoin)
            })
            .collect();
        home.state = State::Cubes {
            cubes,
            create_cube: false,
            source: ChainId::Bitcoin,
        };
        home.advisory_notice = None;
        home.catalog.mode = launcher_catalog::LauncherMode::Cubes;
        home.theme_mode = if light {
            coincube_ui::theme::palette::ThemeMode::Light
        } else {
            coincube_ui::theme::palette::ThemeMode::Dark
        };
        home
    }

    #[derive(Default)]
    struct Probe {
        focus: Vec<(Id, Rectangle)>,
        text: Vec<(String, Rectangle)>,
    }

    impl Operation for Probe {
        fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
            operate(self);
        }
        fn focusable(&mut self, id: Option<&Id>, bounds: Rectangle, _: &mut dyn Focusable) {
            if let Some(id) = id {
                self.focus.push((id.clone(), bounds));
            }
        }
        fn text(&mut self, _: Option<&Id>, bounds: Rectangle, value: &str) {
            self.text.push((value.to_owned(), bounds));
        }
    }

    fn probe<M>(mut element: coincube_ui::widget::Element<'_, M>) -> (Rectangle, Probe) {
        for bytes in coincube_ui::font::load() {
            iced::advanced::graphics::text::font_system()
                .write()
                .unwrap()
                .load_font(bytes);
        }
        let renderer = iced::futures::executor::block_on(<iced::Renderer as Headless>::new(
            coincube_ui::font::REGULAR,
            iced::Pixels(14.0),
            Some("tiny-skia"),
        ))
        .unwrap();
        let mut tree = Tree::new(element.as_widget());
        let node = element.as_widget_mut().layout(
            &mut tree,
            &renderer,
            &layout::Limits::new(Size::ZERO, Size::new(1024.0, 400.0)),
        );
        let mut probe = Probe::default();
        element
            .as_widget_mut()
            .operate(&mut tree, Layout::new(&node), &renderer, &mut probe);
        (node.bounds(), probe)
    }

    /// Import or restore opens on every network. Create and Import close
    /// only when the plan has no Cube slot left, and the reason reads inline
    /// beside them with an Upgrade plan link.
    #[test]
    fn dock_closes_both_entries_only_at_the_plan_limit() {
        let mut home = home(false);
        home.server_cube_limit = Some(10);
        for chain in [ChainId::Bitcoin, ChainId::Testnet4] {
            home.network = chain;
            let projection = launcher_catalog::project(&home);
            assert_eq!(super::plan_full(&projection), None);
            let (_, live) = probe(super::dock(&home, &projection, false));
            assert!(live
                .text
                .iter()
                .any(|(value, _)| value == "Import or restore Bitcoin wallet"));
            assert!(!live.text.iter().any(|(value, _)| value == "Upgrade plan"));
        }

        home.network = ChainId::Bitcoin;
        home.server_cube_limit = Some(4);
        let projection = launcher_catalog::project(&home);
        let caption = super::plan_full(&projection).unwrap();
        assert_eq!(
            caption,
            format!(
                "Bitcoin mainnet: {} of 4 Cubes on the {} plan",
                projection.total_count, projection.tier
            )
        );
        let (_, live) = probe(super::dock(&home, &projection, false));
        let at = |value: &str| {
            live.text
                .iter()
                .find(|(text, _)| text == value)
                .map(|(_, bounds)| *bounds)
        };
        let import = at("Import or restore Bitcoin wallet").unwrap();
        let shown = at(&caption).unwrap();
        assert!(
            shown.x > import.x + import.width,
            "{:?} {:?}",
            import,
            shown
        );
        assert!(at("Upgrade plan").is_some());
    }

    /// The Network menu counts Connect-only Cubes beside this device's, so a
    /// network at its limit adds up to the dock's "N of N", on one line even
    /// for the longest network name.
    #[test]
    fn network_menu_counts_cubes_only_in_connect() {
        let mut home = home(false);
        home.server_cube_limit = Some(7);
        home.remote_cubes = ["r1", "r2", "r3"]
            .iter()
            .map(|uuid| super::super::RemoteCube {
                id: 42,
                uuid: (*uuid).to_string(),
                name: format!("Remote {uuid}"),
                network: crate::app::settings::network_to_api_string(ChainId::Bitcoin),
                has_recovery_kit: true,
                has_encrypted_seed: true,
                phone_recoverable: false,
                phone_full_cube: false,
                encryption_pubkey: None,
                has_vault: None,
            })
            .collect();
        home.catalog.network_menu = true;
        let projection = launcher_catalog::project(&home);
        assert_eq!(
            super::plan_full(&projection).unwrap(),
            format!(
                "Bitcoin mainnet: 7 of 7 Cubes on the {} plan",
                projection.tier
            )
        );
        assert_eq!(
            super::network_menu_count(&projection, ChainId::Bitcoin, 4),
            "4 + 3 in Connect"
        );
        assert_eq!(
            super::network_menu_count(&projection, ChainId::Testnet4, 2),
            "2"
        );

        let (_, live) = probe(super::network_layer(&home, &projection).unwrap());
        let at = |value: &str| {
            live.text
                .iter()
                .find(|(text, _)| text == value)
                .map(|(_, bounds)| *bounds)
                .unwrap_or_else(|| panic!("{:?} not shown", value))
        };
        let name = at("Bitcoin mainnet");
        let count = at("4 + 3 in Connect");
        assert!(count.height <= name.height + 1.0, "{:?} {:?}", name, count);
        assert!(count.x > name.x + name.width, "{:?} {:?}", name, count);
    }

    /// App preferences is icon-only on both Launcher bars, and its accessible
    /// label stays "App preferences". The right group reads Connect, then
    /// App preferences, packed to the 24px gutter. The Launcher shows no
    /// balances, so it has no hide-balances eye; that stays in open Cubes.
    #[test]
    fn launcher_bars_take_the_shared_layout_and_label_the_preferences_icon() {
        let home = home(false);
        let projection = launcher_catalog::project(&home);
        let (bar, live) = probe(super::top_bar(&home, &projection, false));
        assert_eq!(bar.height, super::BAR_HEIGHT);
        let at = |id: &Id| {
            live.focus
                .iter()
                .find(|(target, _)| target == id)
                .map(|(_, bounds)| *bounds)
                .unwrap()
        };
        let settings = at(&launcher_catalog::settings_id());
        assert_eq!((settings.width, settings.height), (32.0, 32.0));
        assert_eq!(settings.x + settings.width, 1024.0 - super::GUTTER);
        let labels: Vec<_> = live
            .text
            .iter()
            .filter(|(value, _)| value == "App preferences")
            .collect();
        assert_eq!(labels.len(), 1, "{:?}", live.text);
        assert_eq!(labels[0].1, settings);
        // A session still being checked leaves the Connect entry inert.
        let connect = live
            .text
            .iter()
            .find(|(value, _)| value == "Connect")
            .unwrap()
            .1;
        assert!(connect.x < settings.x);
        assert!(!live
            .text
            .iter()
            .any(|(value, _)| value.to_lowercase().contains("balances")));
        let brand = live
            .text
            .iter()
            .find(|(value, _)| value == "Tenshu")
            .unwrap()
            .1;
        // The 20px cube and its 8px gap sit at the gutter.
        assert_eq!(brand.x, super::GUTTER + 28.0);

        let chrome = super::snapshot(&home, &projection);
        let (bar, quiet) = probe(super::quiet_top_bar::<()>(&chrome));
        assert_eq!(bar.height, super::BAR_HEIGHT);
        assert!(quiet.focus.is_empty());
        let label = quiet
            .text
            .iter()
            .find(|(value, _)| value == "App preferences")
            .unwrap()
            .1;
        assert_eq!((label.width, label.height), (32.0, 32.0));
        assert_eq!(label.x + label.width, 1024.0 - super::GUTTER);
        assert!(!quiet
            .text
            .iter()
            .any(|(value, _)| value.to_lowercase().contains("balances")));
    }

    #[test]
    #[ignore = "writes PNG frames; needs a GPU adapter"]
    fn top_bar_capture_launcher() {
        let Some(directory) = directory() else {
            return;
        };
        let mut renderer = renderer();
        for (appearance, palette) in themes() {
            let mut home = home(appearance == "light");
            // Signed out, so the Connect entry is the Sign in control. A
            // session still being checked leaves it inert and unfocusable.
            home.connect_account.step = ConnectFlowStep::Login {
                email: String::new(),
                loading: false,
            };
            for (width, height) in WINDOWS {
                save(
                    &mut renderer,
                    launcher_catalog::launcher_view(&home),
                    Size::new(width, height),
                    &palette,
                    None,
                    directory.join(format!("launcher-{appearance}-{width}x{height}.png")),
                );
            }
            for (id, name) in [
                (launcher_catalog::connect_id(), "connect"),
                (launcher_catalog::settings_id(), "preferences"),
            ] {
                save(
                    &mut renderer,
                    launcher_catalog::launcher_view(&home),
                    Size::new(1024.0, 768.0),
                    &palette,
                    Some(id),
                    directory.join(format!("launcher-{appearance}-focus-{name}-1024x768.png")),
                );
            }
            // The live bar over the quiet one the unlock face stands on.
            let projection = launcher_catalog::project(&home);
            let chrome = super::snapshot(&home, &projection);
            let bars = iced::widget::column![
                super::top_bar(&home, &projection, false),
                rule::horizontal(1),
                super::quiet_top_bar(&chrome),
                rule::horizontal(1),
            ];
            save(
                &mut renderer,
                bars.into(),
                Size::new(1024.0, 2.0 * (super::BAR_HEIGHT + 1.0)),
                &palette,
                None,
                directory.join(format!(
                    "launcher-{appearance}-live-and-quiet-bars-1024.png"
                )),
            );
        }
    }
}

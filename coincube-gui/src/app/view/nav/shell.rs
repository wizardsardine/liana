//! The single text sidebar shared by every open-Cube panel.
//!
//! Keep routes in the existing `Menu` model while screen families migrate.
//! Settings and Tools each have one home here; old destination handlers stay
//! reachable without rendering a second or third rail.

use super::{marketplace, NavContext, SubItem};
use crate::{
    app::{
        features,
        menu::{CubeSettingsOption, CubeSubMenu, LiquidSubMenu, Menu, SparkSubMenu, VaultSubMenu},
        settings::preferences::PreferenceChange,
        state::preferences::{BalancePrivacy, BalancePrivacyChrome},
        view::{settings::page, shared::glyph, Message},
        ConnectLight, ConnectionStatus,
    },
    chain::{ChainId, ChainIdExt},
};
use coincube_ui::locale::copy as localized_copy;
use coincube_ui::{
    component::tenshu::top_bar as bar,
    font,
    icon::{cube_icon, shop_icon},
    theme,
    widget::{action::Action, floating::Floating, Column, ColumnExt, Element, Row, Text},
};
use iced::{
    advanced::widget::Id,
    widget::{column, container, row, rule, scrollable, text, Space},
    Alignment, Length,
};

/// Connect live updates and device registration (Cube Settings → Connections).
pub const CONNECT_CONNECTIONS_ROUTE: Menu =
    Menu::Cube(CubeSubMenu::Settings(CubeSettingsOption::Connections));

/// Whether `menu` is a Settings route, whichever scope it is in. The
/// sidebar then selects Settings rather than the wallet the scope names.
pub fn is_settings(menu: &Menu) -> bool {
    matches!(
        menu,
        Menu::Cube(CubeSubMenu::Settings(_))
            | Menu::Vault(VaultSubMenu::Settings(_))
            | Menu::Spark(SparkSubMenu::Settings(_))
            | Menu::Liquid(LiquidSubMenu::Settings(_))
    )
}

pub const FULL_WIDTH: f32 = 248.0;
pub const COMPACT_WIDTH: f32 = 208.0;
pub const TOP_BAR_HEIGHT: f32 = bar::HEIGHT;
pub const LAUNCHER_RETURN_ID: &str = "tenshu.launcher.return";
pub const CUBE_SWITCHER_ID: &str = "tenshu.cube.switcher";
pub const CUBE_LOCK_ID: &str = "tenshu.cube.lock.ask";
pub const CUBE_LOCK_CANCEL_ID: &str = "tenshu.cube.lock.cancel";
pub const SETTINGS_ID: &str = "tenshu.shell.settings";
/// The Connect control. One id in every state, so a status change while it
/// has focus keeps focus on it.
pub const CONNECTION_ID: &str = "tenshu.connect.status";
pub const CONNECT_PANEL_ACCOUNT_ID: &str = "tenshu.connect.panel.account";
pub const CONNECT_PANEL_SETTINGS_ID: &str = "tenshu.connect.panel.settings";
pub const CONNECT_PANEL_SIGN_IN_ID: &str = "tenshu.connect.panel.sign-in";
pub const CONNECT_PANEL_NOT_NOW_ID: &str = "tenshu.connect.panel.not-now";
pub const CONNECT_PANEL_TRY_AGAIN_ID: &str = "tenshu.connect.panel.try-again";
pub const MIDDLE_SCROLL_ID: &str = "tenshu.shell.middle-scroll";
pub const BALANCE_PRIVACY_ID: &str = "tenshu.balance-privacy.toggle";
pub const APP_PREFERENCES_ID: &str = "tenshu.shell.app-preferences";

/// Innermost shell dismissal. The GUI owns Escape dispatch; this helper only
/// identifies the existing harmless intent for the currently open disclosure.
pub fn escape_intent(state: ShellState) -> Option<Message> {
    if state.connect_open {
        Some(Message::CloseConnectPanel)
    } else if state.lock_confirmation {
        Some(Message::CancelCubeLock)
    } else if state.switcher_open {
        Some(Message::ToggleCubeSwitcher)
    } else {
        None
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ShellState {
    pub switcher_open: bool,
    /// The Connect control's panel is open.
    pub connect_open: bool,
    pub lock_confirmation: bool,
    pub backup_attention: bool,
}

pub(crate) fn network_label(network: ChainId) -> &'static str {
    match network {
        ChainId::Bitcoin => "Bitcoin mainnet",
        ChainId::BitcoinBlake2b => "Bitcoin Blake2b mainnet",
        other => other.label(),
    }
}

/// The Launcher's coin for the Cube's chain, beside its network name.
fn network_coin<'a>(ctx: &NavContext<'a>) -> Element<'a, Message> {
    let light = matches!(
        ctx.theme_mode,
        coincube_ui::theme::palette::ThemeMode::Light
    );
    iced::widget::Svg::new(iced::widget::svg::Handle::from_memory(
        crate::home::launcher_coin::widget_source(ctx.network, 14.0, 1.0, light).into_bytes(),
    ))
    .width(Length::Fixed(14.0))
    .height(Length::Fixed(14.0))
    .into()
}

pub(crate) fn short_cube_name(name: &str) -> String {
    // Names are persisted without a length limit and may contain line breaks.
    // Keep fixed sidebar regions to one short, predictable label. The cut
    // counts graphemes, so an accent, an emoji sequence or a flag is never
    // split from the rest of its character.
    use unicode_segmentation::UnicodeSegmentation;
    let flat: String = name
        .chars()
        .map(|ch| {
            if ch.is_whitespace() || ch.is_control() {
                ' '
            } else {
                ch
            }
        })
        .collect();
    let mut graphemes = flat.graphemes(true);
    let prefix: String = graphemes.by_ref().take(16).collect();
    if graphemes.next().is_some() {
        format!("{prefix}…")
    } else {
        prefix
    }
}

fn muted<'a>(label: impl Into<String>, size: u32) -> Text<'a> {
    text(label.into())
        .size(size)
        .style(|t: &theme::Theme| iced::widget::text::Style {
            color: Some(t.colors.shell.muted),
        })
}

use crate::app::view::shared::connect_control::{
    self as shared_connect, chip_button, ConnectDetail, CHIP_RADIUS,
};

/// Legacy entry point for callers without the GUI privacy owner. Cube routes
/// mount `top_bar_with_privacy` from the shared dashboard shell.
pub fn top_bar<'a>(ctx: &NavContext<'a>) -> Element<'a, Message> {
    top_bar_with_privacy(ctx, None)
}

pub fn balance_privacy_label(chrome: BalancePrivacyChrome) -> &'static str {
    match (chrome.busy, chrome.privacy, chrome.intent().is_some()) {
        (true, _, _) => localized_copy("Saving balance privacy…"),
        (false, BalancePrivacy::Unresolved, true) => {
            localized_copy("Balances hidden for now. Show balances")
        }
        (false, BalancePrivacy::Unresolved, false) => {
            localized_copy("Balances hidden until your choice loads")
        }
        (false, BalancePrivacy::Hidden, true) => localized_copy("Show balances"),
        (false, BalancePrivacy::Hidden, false) => localized_copy("Balances hidden"),
        (false, BalancePrivacy::HiddenLocally, true) => {
            localized_copy("Balances hidden now. Retry Hide")
        }
        (false, BalancePrivacy::HiddenLocally, false) => localized_copy("Balances hidden now"),
        (false, BalancePrivacy::Shown, true) => localized_copy("Hide balances"),
        (false, BalancePrivacy::Shown, false) => localized_copy("Balances shown"),
    }
}

/// The balances eye on every bar: shut while amounts are masked, pressed
/// while the choice is to hide them, and inert while a change is saving. It
/// emits only the existing device-preference intent.
pub fn balance_privacy_eye<'a, M: Clone + 'static>(
    id: impl Into<Id>,
    chrome: BalancePrivacyChrome,
    focus_intent: fn(Id) -> M,
    on_change: fn(PreferenceChange) -> M,
) -> bar::IconAction<'a, M> {
    let glyph = if chrome.privacy.masks_amounts() {
        bar::EYE_SHUT
    } else {
        bar::EYE_OPEN
    };
    bar::icon_action(id, glyph, balance_privacy_label(chrome), focus_intent)
        .on_press_maybe(chrome.intent().map(on_change))
        .pressed(matches!(
            chrome.privacy,
            BalancePrivacy::Hidden | BalancePrivacy::HiddenLocally
        ))
}

pub fn top_bar_with_privacy<'a>(
    ctx: &NavContext<'a>,
    privacy: Option<BalancePrivacyChrome>,
) -> Element<'a, Message> {
    let connect = connect_control(ctx);
    let top: Element<'a, Message> = bar::top_bar(
        bar::brand_action(
            LAUNCHER_RETURN_ID,
            Message::FocusAction,
            Message::OpenLauncher,
        ),
        bar::preferences(
            APP_PREFERENCES_ID,
            Message::FocusAction,
            Message::OpenAppPreferences,
        ),
    )
    .eye(privacy.map(|chrome| {
        balance_privacy_eye(
            BALANCE_PRIVACY_ID,
            chrome,
            Message::FocusAction,
            Message::ChangeDevicePreference,
        )
    }))
    .chip(connect)
    .into();
    // `.top { border-bottom: 1px }`, under the bar as on the Launcher.
    let mut bar = column![top, rule::horizontal(1)];
    // The panel hangs from the control's trailing edge, clear of App
    // preferences, and covers the page rather than pushing it down.
    if ctx.connect.panel_open {
        bar = bar.push(
            Floating::new(
                container(connect_panel(ctx)).padding(iced::Padding::default().right(64.0)),
            )
            .align_x(iced::alignment::Horizontal::Right)
            .gap(6.0)
            .on_dismiss(Message::CloseConnectPanel),
        );
    }
    bar.into()
}

/// The light Connect shows: none while signed out by the user, red while
/// signed out by Connect, otherwise the live-updates light.
fn connect_light(ctx: &NavContext<'_>) -> Option<ConnectLight> {
    if !ctx.connect_authenticated {
        return ctx.connect.signed_out_by.map(|_| ConnectLight::NeedsYou);
    }
    ctx.connect_stream_status.light()
}

/// The control's accessible name: the account, plus what its light means.
fn connect_label(ctx: &NavContext<'_>) -> String {
    if !ctx.connect_authenticated {
        return match ctx.connect.signed_out_by {
            Some(crate::app::SignedOutBy::ComputerRemoved) => "Connect: Sign in, needs attention: \
                 this computer was removed from your Connect account"
                .to_owned(),
            Some(crate::app::SignedOutBy::SessionEnded) => {
                "Connect: Sign in, needs attention: your Connect session ended".to_owned()
            }
            None => "Connect: Sign in".to_owned(),
        };
    }
    let account = match ctx.connect_email {
        Some(email) => format!("Connect account, {email}"),
        None => "Connect account".to_owned(),
    };
    let meaning = match ctx.connect_stream_status {
        ConnectionStatus::Inactive => return account,
        ConnectionStatus::Connected => localized_copy("live updates on"),
        ConnectionStatus::Connecting => "connecting",
        ConnectionStatus::Reconnecting | ConnectionStatus::Renewing => "reconnecting",
        ConnectionStatus::Offline => localized_copy("offline: no internet connection"),
        ConnectionStatus::RenewFailed => {
            localized_copy("needs attention: couldn’t reconnect this computer")
        }
    };
    format!("{account}, {meaning}")
}

/// The Connect control (`Connect · {email}`, `Connect · Offline` or
/// `Connect · Sign in`): one control for the account and this computer's
/// live updates, with a status light on the account glyph.
fn connect_control<'a>(ctx: &NavContext<'a>) -> Element<'a, Message> {
    let signed_in = ctx.connect_authenticated;
    let has_panel = signed_in || ctx.connect.signed_out_by.is_some();
    let detail = if !signed_in {
        ConnectDetail::SignIn
    } else if *ctx.connect_stream_status == ConnectionStatus::Offline {
        ConnectDetail::Offline
    } else {
        ConnectDetail::Account(ctx.connect_email.unwrap_or("Signed in"))
    };
    shared_connect::connect_control(
        shared_connect::ConnectControl {
            id: Id::new(CONNECTION_ID),
            detail,
            light: connect_light(ctx),
            light_dim: ctx.connect.light_dim,
            chevron: has_panel,
            open: ctx.connect.panel_open,
            label: connect_label(ctx),
        },
        Some(if has_panel {
            Message::ToggleConnectPanel
        } else {
            Message::OpenConnectSignIn
        }),
        Message::FocusAction,
    )
}

/// Which panel control takes focus when the panel opens: the notice's first
/// action when there is one, otherwise Connect account.
pub fn connect_panel_first_focus(cache: &crate::app::cache::Cache) -> Id {
    if !cache.connect_authenticated && cache.connect_signed_out_by.is_some() {
        Id::from(CONNECT_PANEL_SIGN_IN_ID)
    } else if cache.connect_stream_status == ConnectionStatus::RenewFailed {
        Id::from(CONNECT_PANEL_TRY_AGAIN_ID)
    } else {
        Id::from(CONNECT_PANEL_ACCOUNT_ID)
    }
}

/// A footer link in the panel (`.mi`): a label and a chevron.
fn panel_link<'a>(
    id: &'static str,
    label: &'static str,
    on_press: Message,
) -> Element<'a, Message> {
    Action::new(
        id,
        row![
            text(label).size(13).width(Length::Fill),
            glyph::glyph(glyph::CHEVRON_RIGHT, 16.0, glyph::muted),
        ]
        .align_y(Alignment::Center),
        Message::FocusAction,
    )
    .width(Length::Fill)
    .padding([9, 10])
    .style(chip_button)
    .focus_radius(CHIP_RADIUS)
    .on_press(on_press)
    .into()
}

fn rule_line<'a>() -> Element<'a, Message> {
    rule::horizontal(1).into()
}

/// The Connect panel: the account, live updates on this computer, a notice
/// only when the user has something to do, and a Vault line only when a
/// Vault that uses Connect has stopped updating.
fn connect_panel<'a>(ctx: &NavContext<'a>) -> Element<'a, Message> {
    let signed_in = ctx.connect_authenticated;
    let account = ctx
        .connect_email
        .filter(|_| signed_in)
        .unwrap_or("Connect account");
    let head = row![
        container(glyph::glyph(glyph::USER, 18.0, glyph::accent))
            .center_x(Length::Fixed(36.0))
            .center_y(Length::Fixed(36.0))
            .style(|t: &theme::Theme| iced::widget::container::Style {
                background: Some(t.colors.shell.background.into()),
                border: iced::Border {
                    radius: 18.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            }),
        column![
            text(account)
                .size(14)
                .font(font::SEMIBOLD)
                .wrapping(text::Wrapping::WordOrGlyph),
            muted(
                localized_copy(if signed_in {
                    "Signed in to COINCUBE | Connect"
                } else {
                    "Signed out of COINCUBE | Connect"
                }),
                12,
            ),
        ]
        .spacing(2),
    ]
    .spacing(12)
    .align_y(Alignment::Center);

    let mut body = Column::new().spacing(6);
    let mut foot: Vec<Element<'a, Message>> = Vec::new();
    match ctx.connect.signed_out_by.filter(|_| !signed_in) {
        Some(reason) => {
            body = body
                .push(text(reason.title()).size(14).font(font::SEMIBOLD))
                .push(muted(reason.detail(), 13))
                .push(Space::new().height(Length::Fixed(8.0)))
                .push(
                    row![
                        page::primary(
                            CONNECT_PANEL_SIGN_IN_ID,
                            "Sign in",
                            Some(Message::OpenConnectSignIn)
                        ),
                        page::button(
                            CONNECT_PANEL_NOT_NOW_ID,
                            "Not now",
                            Some(Message::DismissConnectSignOut)
                        ),
                    ]
                    .spacing(8),
                );
        }
        None => {
            let status = *ctx.connect_stream_status;
            let kind = match status {
                ConnectionStatus::Connected => page::Status::Ok,
                ConnectionStatus::Inactive | ConnectionStatus::Connecting => page::Status::Busy,
                ConnectionStatus::Reconnecting | ConnectionStatus::Renewing => page::Status::Warn,
                ConnectionStatus::Offline | ConnectionStatus::RenewFailed => page::Status::Off,
            };
            body = body.push(
                row![
                    text(localized_copy("Live updates on this computer"))
                        .size(13)
                        .font(font::MEDIUM)
                        .width(Length::Fill),
                    page::status(kind, localized_copy(status.word())),
                ]
                .spacing(12)
                .align_y(Alignment::Center),
            );
            if let Some(line) = status.line() {
                body = body.push(muted(line, 13));
            }
            if status == ConnectionStatus::RenewFailed {
                body = body
                    .push(Space::new().height(Length::Fixed(6.0)))
                    .push(rule_line())
                    .push(Space::new().height(Length::Fixed(6.0)))
                    .push(
                        text(crate::app::RENEW_FAILED_TITLE)
                            .size(14)
                            .font(font::SEMIBOLD),
                    )
                    .push(muted(crate::app::renew_failed_detail(), 13))
                    .push(Space::new().height(Length::Fixed(8.0)))
                    .push(page::primary(
                        CONNECT_PANEL_TRY_AGAIN_ID,
                        localized_copy("Try again"),
                        Some(Message::Settings(
                            crate::app::view::SettingsMessage::ReregisterConnectDevice,
                        )),
                    ));
            } else {
                foot.push(panel_link(
                    CONNECT_PANEL_ACCOUNT_ID,
                    coincube_ui::locale::copy("Connect account"),
                    Message::OpenConnectSwitchAccount,
                ));
            }
            foot.push(panel_link(
                CONNECT_PANEL_SETTINGS_ID,
                coincube_ui::locale::copy("Connections settings"),
                Message::Menu(CONNECT_CONNECTIONS_ROUTE),
            ));
        }
    }
    if let Some(alert) = ctx.connect.vault_alert.filter(|a| a.via_connect) {
        body = body
            .push(Space::new().height(Length::Fixed(6.0)))
            .push(rule_line())
            .push(Space::new().height(Length::Fixed(6.0)))
            .push(
                row![
                    row![
                        glyph::glyph(glyph::SHIELD, 16.0, glyph::muted),
                        text(wallet_label("Vault", ctx.vault_alias))
                            .size(13)
                            .font(font::MEDIUM),
                    ]
                    .spacing(8)
                    .align_y(Alignment::Center)
                    .width(Length::Fill),
                    vault_status(alert),
                ]
                .spacing(12)
                .align_y(Alignment::Center),
            )
            .push(muted(alert.panel_line(), 13));
    }

    let mut panel = column![
        container(head).padding([14, 16]),
        rule_line(),
        container(body).padding([14, 16]),
    ];
    if !foot.is_empty() {
        panel = panel
            .push(rule_line())
            .push(container(Column::with_children(foot).spacing(2)).padding(6));
    }
    container(panel)
        .width(Length::Fixed(360.0))
        .style(theme::container::shell_menu)
        .into()
}

/// The Vault's status word while it isn't updating: Offline grey,
/// Reconnecting… and Not updating amber.
pub fn vault_status<'a>(alert: &crate::app::VaultAlert) -> Element<'a, Message> {
    let kind = match alert.stall {
        crate::app::VaultStall::Offline => page::Status::Off,
        _ => page::Status::Warn,
    };
    page::status(kind, alert.word())
}

/// A rail icon: the canvas's stroke glyph, or a font icon where the canvas
/// draws none (Marketplace).
enum NavIcon {
    Glyph(&'static str),
    Font(Text<'static>),
}

/// A wallet row's label: `Spark · Daily spending`, or plain `Spark` while the
/// wallet has no name. The name is cut like the Cube name above it.
pub(crate) fn wallet_label(wallet: &str, name: Option<&str>) -> String {
    match name.map(str::trim).filter(|name| !name.is_empty()) {
        Some(name) => format!("{wallet} · {}", short_cube_name(name)),
        None => wallet.to_owned(),
    }
}

fn nav_row<'a>(
    label: impl Into<String>,
    icon: Option<NavIcon>,
    active: bool,
    message: Option<Message>,
    nested: bool,
    disabled_reason: Option<String>,
) -> Element<'a, Message> {
    let key = label.into();
    let label = localized_copy(&key).to_owned();
    nav_row_with_key(key, label, icon, active, message, nested, disabled_reason)
}

/// A top-level row with the Vault's alert dot after its label.
fn nav_row_alert<'a>(
    key: String,
    label: String,
    icon: Option<NavIcon>,
    active: bool,
    message: Option<Message>,
    alert: bool,
) -> Element<'a, Message> {
    nav_row_inner(key, label, icon, active, message, false, None, alert)
}

/// A row whose focus ID comes from `key`, not its label, so a label that
/// carries a user-chosen name keeps one ID whatever the name.
fn nav_row_with_key<'a>(
    key: String,
    label: String,
    icon: Option<NavIcon>,
    active: bool,
    message: Option<Message>,
    nested: bool,
    disabled_reason: Option<String>,
) -> Element<'a, Message> {
    nav_row_inner(
        key,
        label,
        icon,
        active,
        message,
        nested,
        disabled_reason,
        false,
    )
}

#[allow(clippy::too_many_arguments)]
fn nav_row_inner<'a>(
    key: String,
    label: String,
    icon: Option<NavIcon>,
    active: bool,
    message: Option<Message>,
    nested: bool,
    disabled_reason: Option<String>,
    alert: bool,
) -> Element<'a, Message> {
    let mut body: Row<Message> = Row::new().spacing(10).align_y(Alignment::Center);
    if nested {
        body = body.push(Space::new().width(Length::Fixed(18.0)));
    }
    // `.ni .ic` is muted; `.ni.on .ic` takes the accent ink.
    let tint = if active { glyph::accent } else { glyph::muted };
    match icon {
        Some(NavIcon::Glyph(source)) => body = body.push(glyph::glyph(source, 18.0, tint)),
        Some(NavIcon::Font(icon)) => {
            body = body.push(icon.size(17).width(Length::Fixed(18.0)).style(
                move |t: &theme::Theme| iced::widget::text::Style {
                    color: Some(tint(t)),
                },
            ))
        }
        None => {}
    }
    // `.ni.on { font-weight: 600 }`. `.ni .lbl { overflow-wrap: anywhere }`:
    // a long wallet name wraps inside the row rather than running past it.
    body = body.push(
        text(label)
            .size(if nested { 13 } else { 14 })
            .font(if active {
                font::SEMIBOLD
            } else {
                font::REGULAR
            })
            .width(Length::Fill)
            .wrapping(text::Wrapping::WordOrGlyph),
    );
    // `.ni .nd`: the Vault's alert dot, in the warning colour.
    if alert {
        body = body.push(
            container(Space::new())
                .width(Length::Fixed(8.0))
                .height(Length::Fixed(8.0))
                .style(|t: &theme::Theme| iced::widget::container::Style {
                    background: Some(t.colors.shell.warn.into()),
                    border: iced::Border {
                        radius: 4.0.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                }),
        );
    }
    let id = if nested {
        format!("tenshu.shell.sub.{key}.{message:?}")
    } else {
        match key.as_str() {
            "Lock Cube" => CUBE_LOCK_ID.to_owned(),
            "Settings" => SETTINGS_ID.to_owned(),
            "All Cubes" => "tenshu.launcher.all-cubes".to_owned(),
            _ => format!("tenshu.shell.nav.{key}"),
        }
    };
    let mut button = Action::new(id, body, Message::FocusAction)
        .width(Length::Fill)
        .padding([9, 10])
        .style(if active {
            theme::button::shell_nav_active
        } else {
            theme::button::shell_nav
        });
    if let Some(message) = message {
        button = button.on_press(message);
    }
    let row: Element<'a, Message> = button.into();
    if let Some(reason) = disabled_reason {
        iced::widget::tooltip(
            row,
            container(muted(localized_copy(&reason), 12))
                .padding([6, 10])
                .style(theme::container::shell_menu),
            iced::widget::tooltip::Position::Right,
        )
        .gap(6)
        .into()
    } else {
        row
    }
}

fn route<'a>(
    label: impl Into<String>,
    icon: Option<NavIcon>,
    menu: &Menu,
    to: Menu,
    nested: bool,
) -> Element<'a, Message> {
    let active = menu == &to;
    nav_row(label, icon, active, Some(Message::Menu(to)), nested, None)
}

fn subitem<'a>(menu: &Menu, item: SubItem, label: Option<&str>) -> Element<'a, Message> {
    let active = item.disabled_reason.is_none() && (item.matches)(menu);
    let disabled_reason = item.disabled_reason;
    nav_row(
        label.unwrap_or(item.label),
        None,
        active,
        disabled_reason
            .is_none()
            .then_some(Message::Menu(item.route)),
        true,
        disabled_reason,
    )
}

/// `.gh`: a 13px semibold muted group heading.
fn section(label: &'static str) -> Element<'static, Message> {
    container(muted(label, 13).font(font::SEMIBOLD))
        .padding(iced::Padding {
            top: 22.0,
            right: 10.0,
            bottom: 6.0,
            left: 10.0,
        })
        .into()
}

fn identity<'a>(ctx: &NavContext<'a>) -> Element<'a, Message> {
    let avatar: Element<'a, Message> = if let Some(handle) = ctx.avatar {
        iced::widget::image(handle.clone())
            .width(Length::Fixed(36.0))
            .height(Length::Fixed(36.0))
            .into()
    } else {
        container(cube_icon().size(20))
            .width(Length::Fixed(36.0))
            .height(Length::Fixed(36.0))
            .center_x(Length::Fixed(36.0))
            .center_y(Length::Fixed(36.0))
            .style(theme::container::shell_menu)
            .into()
    };
    let title = if ctx.cube_name.is_empty() {
        "Cube".to_owned()
    } else {
        short_cube_name(ctx.cube_name)
    };
    let control = Action::new(
        CUBE_SWITCHER_ID,
        row![
            avatar,
            column![
                text(title)
                    .size(14)
                    .wrapping(iced::widget::text::Wrapping::WordOrGlyph),
                // `.net { white-space: nowrap }`: the coin and the name stay on one line.
                row![
                    network_coin(ctx),
                    muted(network_label(ctx.network), 12)
                        .wrapping(iced::widget::text::Wrapping::None)
                ]
                .spacing(5)
                .align_y(Alignment::Center)
            ]
            .spacing(1)
            .width(Length::Fill),
            // `.side .cv`: one muted 16px chevron; the open menu shows the state.
            glyph::glyph(glyph::CHEVRON_DOWN, 16.0, glyph::muted),
        ]
        .spacing(9)
        .align_y(Alignment::Center),
        Message::FocusAction,
    )
    .padding([8, 10])
    .width(Length::Fill)
    .style(theme::button::shell_nav)
    .on_press(Message::ToggleCubeSwitcher);
    container(control).padding([4, 8]).into()
}

fn switcher_menu<'a>(ctx: &NavContext<'a>, state: ShellState) -> Element<'a, Message> {
    let mut items: Column<Message> = column![nav_row(
        "All Cubes",
        Some(NavIcon::Glyph(glyph::GRID)),
        false,
        Some(Message::OpenLauncher),
        false,
        None,
    )]
    .spacing(4);
    if state.lock_confirmation {
        let prompt = format!(
            "Lock {} and open the Launcher?",
            short_cube_name(ctx.cube_name)
        );
        let full_name = container(
            scrollable(
                muted(ctx.cube_name, 12).wrapping(iced::widget::text::Wrapping::WordOrGlyph),
            )
            .width(Length::Fixed(200.0))
            .height(Length::Fixed(120.0)),
        )
        .padding([6, 10])
        .style(theme::container::shell_menu);
        items = items
            .push(iced::widget::tooltip(
                muted(prompt, 13).wrapping(iced::widget::text::Wrapping::WordOrGlyph),
                full_name,
                iced::widget::tooltip::Position::Right,
            ))
            .push(
                row![
                    container(
                        Action::new(
                            "tenshu.cube.lock.confirm",
                            text("Lock").size(13),
                            Message::FocusAction
                        )
                        .padding([6, 12])
                        .style(theme::button::primary)
                        .on_press(Message::LockCubeToLauncher)
                    )
                    .id("cube-lock-confirm"),
                    container(
                        Action::new(
                            CUBE_LOCK_CANCEL_ID,
                            text("Cancel").size(13),
                            Message::FocusAction
                        )
                        .padding([6, 12])
                        .style(theme::button::shell_nav)
                        .on_press(Message::CancelCubeLock)
                    )
                    .id("cube-lock-cancel"),
                ]
                .spacing(8),
            );
    } else {
        items = items.push(nav_row(
            "Lock Cube",
            Some(NavIcon::Glyph(glyph::LOCK)),
            false,
            Some(Message::AskCubeLock),
            false,
            None,
        ));
    }
    container(items)
        .padding(7)
        .style(theme::container::shell_menu)
        .into()
}

fn activity_route(_ctx: &NavContext<'_>) -> Option<Menu> {
    Some(Menu::Cube(CubeSubMenu::Activity))
}

/// Marketplace is the one sidebar entry with children (P2P, Buy/Sell), as
/// on the canvas. Wallets have none: Send, Receive and their history are
/// reached from each wallet's own page.
fn marketplace_children<'a>(menu: &Menu, ctx: &NavContext<'a>) -> Vec<Element<'a, Message>> {
    marketplace::items(ctx)
        .into_iter()
        .map(|item| subitem(menu, item, None))
        .collect()
}

pub const BACKUP_ATTENTION_ID: &str = "tenshu.shell.backup-attention";

/// `.attn`: a bordered panel card above the foot, a warning mark and
/// "Backup not confirmed" in the warning ink, with "Review" in the accent.
fn backup_attention<'a>() -> Element<'a, Message> {
    let body = row![
        glyph::glyph(glyph::WARNING, 16.0, glyph::warn),
        text(localized_copy("Backup not confirmed"))
            .size(12.5)
            .font(font::MEDIUM)
            .style(|t: &theme::Theme| iced::widget::text::Style {
                color: Some(t.colors.shell.warn),
            }),
        Space::new().width(Length::Fill),
        text(localized_copy("Review"))
            .size(12.5)
            .font(font::SEMIBOLD)
            .style(|t: &theme::Theme| iced::widget::text::Style {
                color: Some(t.colors.shell.accent_ink),
            }),
    ]
    .spacing(6)
    .align_y(Alignment::Center);
    container(
        Action::new(BACKUP_ATTENTION_ID, body, Message::FocusAction)
            .width(Length::Fill)
            .padding([9, 8])
            .style(|t: &theme::Theme, status| {
                let shell = t.colors.shell;
                let hovered = matches!(
                    status,
                    iced::widget::button::Status::Hovered | iced::widget::button::Status::Pressed
                );
                iced::widget::button::Style {
                    background: Some(iced::Background::Color(if hovered {
                        shell.selected
                    } else {
                        shell.panel
                    })),
                    text_color: shell.ink,
                    border: iced::Border {
                        color: shell.line,
                        width: 1.0,
                        radius: 8.0.into(),
                    },
                    ..Default::default()
                }
            })
            .on_press(Message::Menu(Menu::Cube(CubeSubMenu::Settings(
                CubeSettingsOption::Recovery,
            )))),
    )
    .padding(iced::Padding {
        top: 0.0,
        right: 16.0,
        bottom: 8.0,
        left: 16.0,
    })
    .into()
}

/// One anchored sidebar, with only its middle group scrolling.
pub fn sidebar<'a>(menu: &Menu, ctx: &NavContext<'a>, state: ShellState) -> Element<'a, Message> {
    let overview = Menu::Cube(CubeSubMenu::Overview);
    let backup = Menu::Cube(CubeSubMenu::Settings(CubeSettingsOption::Recovery));
    let activity = activity_route(ctx);
    let activity_unavailable = false;
    // A focused row paints 4px beyond its hit bounds. Later selected rows
    // have opaque fills, so they need this clearance to keep the ring visible.
    let mut top: Column<Message> = Column::new().spacing(4).padding([0, 10]);
    top = top
        .push(route(
            "Overview",
            Some(NavIcon::Glyph(glyph::GRID)),
            menu,
            overview,
            false,
        ))
        .push(nav_row(
            "Activity",
            Some(NavIcon::Glyph(glyph::CLOCK)),
            matches!(menu, Menu::Cube(CubeSubMenu::Activity)),
            activity.map(Message::Menu),
            false,
            activity_unavailable.then(|| "No wallet with activity on this network".to_owned()),
        ))
        .push(route(
            "Backup & protection",
            Some(NavIcon::Glyph(glyph::SHIELD_CHECK)),
            menu,
            backup,
            false,
        ));

    // The scroll viewport clips drawing outside its content. Reserve more
    // than the focus ring's 4px outer extent at its first and last actions.
    let mut middle: Column<Message> = Column::new().spacing(4).padding([6, 10]);
    middle = middle.push(section(localized_copy("Wallets")));
    // Settings scoped to a wallet highlights Settings, not the wallet.
    let settings_active = is_settings(menu);
    if !ctx.network.is_blake2b() {
        let spark_reason = features::spark(ctx.network).reason().map(str::to_owned);
        middle = middle.push(nav_row_with_key(
            "Spark".to_owned(),
            wallet_label("Spark", ctx.spark_alias),
            Some(NavIcon::Glyph(glyph::SPARK_STROKE)),
            matches!(menu, Menu::Spark(_)) && !settings_active,
            spark_reason
                .is_none()
                .then_some(Message::Menu(Menu::Spark(SparkSubMenu::Overview))),
            false,
            spark_reason,
        ));
    }
    if ctx.has_vault || ctx.has_imported_bitcoin {
        let label = wallet_label("Vault", ctx.vault_alias);
        let vault = nav_row_alert(
            "Vault".to_owned(),
            label.clone(),
            Some(NavIcon::Glyph(glyph::SHIELD)),
            matches!(menu, Menu::Vault(_)) && !settings_active,
            Some(Message::Menu(Menu::Vault(VaultSubMenu::Overview))),
            ctx.connect.vault_alert.is_some(),
        );
        // The dot is never the only signal: the name says it too.
        middle = middle.push(match ctx.connect.vault_alert {
            Some(alert) => bar::accessible(
                vault,
                format!(
                    "{label} ({})",
                    alert.word().trim_end_matches('…').to_lowercase()
                ),
            ),
            None => vault,
        });
    } else {
        middle = middle.push(nav_row(
            "Set up Vault",
            Some(NavIcon::Glyph(glyph::SHIELD)),
            false,
            Some(Message::SetupVault),
            false,
            None,
        ));
    }
    if !ctx.network.is_blake2b() && ctx.liquid_gate.show() {
        let liquid_reason = features::liquid(ctx.network).reason().map(str::to_owned);
        middle = middle.push(nav_row(
            "Liquid",
            Some(NavIcon::Glyph(glyph::DROPLET)),
            matches!(menu, Menu::Liquid(_)) && !settings_active,
            liquid_reason
                .is_none()
                .then_some(Message::Menu(Menu::Liquid(LiquidSubMenu::Overview))),
            false,
            liquid_reason,
        ));
    }
    let show_marketplace = (ctx.has_p2p && ctx.marketplace_flags.p2p_on())
        || (ctx.has_vault && ctx.marketplace_flags.buy_sell_on());
    if show_marketplace {
        middle = middle.push(nav_row(
            "Marketplace",
            Some(NavIcon::Font(shop_icon())),
            matches!(menu, Menu::Marketplace(_)),
            Some(Message::Menu(super::primary::marketplace_landing_menu(ctx))),
            false,
            None,
        ));
        if matches!(menu, Menu::Marketplace(_)) {
            for item in marketplace_children(menu, ctx) {
                middle = middle.push(item);
            }
            if let Some(subpages) = super::tertiary::items_for(menu, ctx) {
                for item in subpages {
                    middle = middle.push(subitem(menu, item, None));
                }
            }
        }
    }
    let foot = column![
        rule::horizontal(1),
        container(nav_row(
            "Settings",
            Some(NavIcon::Glyph(glyph::GEAR)),
            settings_active,
            Some(Message::Menu(Menu::Cube(CubeSubMenu::Settings(
                CubeSettingsOption::General
            )))),
            false,
            None
        ))
        .id("cube-sidebar-settings"),
    ]
    .spacing(8)
    .padding([8, 10]);

    let mut sidebar: Column<Message> = Column::new()
        .push(identity(ctx))
        .width(Length::Fill)
        .height(Length::Fill);
    // The menu covers the rows below it rather than pushing them down.
    if state.switcher_open {
        // Escape belongs to the menu while it is open, so a page underneath
        // that also answers Escape (a payment result) keeps its screen.
        sidebar = sidebar.push(
            Floating::new(container(switcher_menu(ctx, state)).padding([0, 8])).on_escape(
                if state.lock_confirmation {
                    Message::CancelCubeLock
                } else {
                    Message::ToggleCubeSwitcher
                },
            ),
        );
    }
    sidebar = sidebar
        .push(top)
        .push(scrollable(middle).id(MIDDLE_SCROLL_ID).height(Length::Fill))
        .push_maybe(state.backup_attention.then(backup_attention))
        .push(foot);
    container(sidebar)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(theme::container::shell_rail)
        .into()
}

/// What the runtime does with one event, for tests that drive the shell
/// without it: overlays (the switcher menu) first, the base only if they
/// leave the event uncaptured, and with no cursor where they cover it.
#[cfg(test)]
pub(crate) fn test_send(
    element: &mut Element<'_, Message>,
    tree: &mut iced::advanced::widget::Tree,
    node: &iced::advanced::layout::Node,
    renderer: &iced::Renderer,
    event: &iced::Event,
    cursor: iced::mouse::Cursor,
    size: iced::Size,
) -> Vec<Message> {
    use iced::advanced::{clipboard::Null, Layout, Shell};
    let mut messages = Vec::new();
    let viewport = iced::Rectangle::with_size(size);
    let mut base_cursor = cursor;
    if let Some(mut overlay) = element.as_widget_mut().overlay(
        tree,
        Layout::new(node),
        renderer,
        &viewport,
        iced::Vector::ZERO,
    ) {
        let overlay = overlay.as_overlay_mut();
        let overlay_node = overlay.layout(renderer, size);
        let mut shell = Shell::new(&mut messages);
        overlay.update(
            event,
            Layout::new(&overlay_node),
            cursor,
            renderer,
            &mut Null,
            &mut shell,
        );
        if shell.is_event_captured() {
            return messages;
        }
        if overlay.mouse_interaction(Layout::new(&overlay_node), cursor, renderer)
            != iced::mouse::Interaction::None
        {
            base_cursor = iced::mouse::Cursor::Unavailable;
        }
    }
    element.as_widget_mut().update(
        tree,
        event,
        Layout::new(node),
        base_cursor,
        renderer,
        &mut Null,
        &mut Shell::new(&mut messages),
        &viewport,
    );
    messages
}

/// Draws the shell and then its overlays on top, as the runtime does.
#[cfg(test)]
pub(crate) fn test_draw(
    element: &mut Element<'_, Message>,
    tree: &mut iced::advanced::widget::Tree,
    node: &iced::advanced::layout::Node,
    renderer: &mut iced::Renderer,
    theme: &theme::Theme,
    size: iced::Size,
) {
    use iced::advanced::{renderer::Style, Layout, Renderer as _};
    let viewport = iced::Rectangle::with_size(size);
    let style = Style {
        text_color: theme.colors.shell.ink,
    };
    element.as_widget().draw(
        tree,
        renderer,
        theme,
        &style,
        Layout::new(node),
        iced::mouse::Cursor::Unavailable,
        &viewport,
    );
    if let Some(mut overlay) = element.as_widget_mut().overlay(
        tree,
        Layout::new(node),
        renderer,
        &viewport,
        iced::Vector::ZERO,
    ) {
        let overlay = overlay.as_overlay_mut();
        let overlay_node = overlay.layout(renderer, size);
        renderer.with_layer(viewport, |renderer| {
            overlay.draw(
                renderer,
                theme,
                &style,
                Layout::new(&overlay_node),
                iced::mouse::Cursor::Unavailable,
            );
        });
    }
}

#[cfg(test)]
pub(crate) mod capture_tests;

#[cfg(test)]
mod keyboard_tests;

#[cfg(test)]
mod privacy_eye_tests {
    use super::*;
    use crate::app::settings::preferences::PreferenceChange;

    #[test]
    fn eye_copy_matches_persisted_intent_and_failed_hide_latch() {
        let chrome = |privacy| BalancePrivacyChrome {
            privacy,
            busy: false,
            interactive: true,
        };
        let shown = chrome(BalancePrivacy::Shown);
        assert_eq!(balance_privacy_label(shown), "Hide balances");
        assert_eq!(shown.intent(), Some(PreferenceChange::HideBalances(true)));

        let hidden = chrome(BalancePrivacy::Hidden);
        assert_eq!(balance_privacy_label(hidden), "Show balances");
        assert_eq!(hidden.intent(), Some(PreferenceChange::HideBalances(false)));

        let failed_hide = chrome(BalancePrivacy::HiddenLocally);
        assert_eq!(
            balance_privacy_label(failed_hide),
            "Balances hidden now. Retry Hide"
        );
        assert_eq!(
            failed_hide.intent(),
            Some(PreferenceChange::HideBalances(true))
        );

        let busy = BalancePrivacyChrome {
            busy: true,
            ..shown
        };
        assert_eq!(busy.intent(), None);
    }
}

#[cfg(test)]
mod short_cube_name_tests {
    use super::short_cube_name;

    /// The 16-character cut counts graphemes: an accent typed as a combining
    /// mark, an emoji sequence and a flag stay whole at the boundary.
    #[test]
    fn the_cut_never_splits_a_grapheme() {
        let a15 = "a".repeat(15);
        // "é" as "e" plus a combining acute accent.
        assert_eq!(
            short_cube_name(&format!("{a15}e\u{301}xyz")),
            format!("{a15}e\u{301}…")
        );
        let family = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}";
        assert_eq!(
            short_cube_name(&format!("{a15}{family}tail")),
            format!("{a15}{family}…")
        );
        let flag = "\u{1F1F2}\u{1F1FD}";
        assert_eq!(
            short_cube_name(&format!("{a15}{flag}tail")),
            format!("{a15}{flag}…")
        );
        // Exactly 16 graphemes need no cut, however many chars they take.
        assert_eq!(
            short_cube_name(&format!("{a15}{family}")),
            format!("{a15}{family}")
        );
        // Line breaks and other spacing still read as one space each.
        assert_eq!(short_cube_name("Every\nday\tCube"), "Every day Cube");
    }
}

#[cfg(test)]
mod settings_selection_tests {
    use super::*;
    use crate::app::menu::{SettingsOption, SparkSettingsOption};

    /// Settings scoped to Spark or Vault is still Settings: the sidebar
    /// selects Settings there, and the wallet row only on the wallet's pages.
    #[test]
    fn wallet_scoped_settings_count_as_settings() {
        for menu in [
            Menu::Cube(CubeSubMenu::Settings(CubeSettingsOption::Connections)),
            Menu::Spark(SparkSubMenu::Settings(Some(
                SparkSettingsOption::LightningAddress,
            ))),
            Menu::Vault(VaultSubMenu::Settings(Some(SettingsOption::Node))),
            Menu::Vault(VaultSubMenu::Settings(None)),
        ] {
            assert!(is_settings(&menu), "{:?}", menu);
        }
        for menu in [
            Menu::Spark(SparkSubMenu::Overview),
            Menu::Vault(VaultSubMenu::Overview),
            Menu::Cube(CubeSubMenu::Overview),
        ] {
            assert!(!is_settings(&menu), "{:?}", menu);
        }
    }
}

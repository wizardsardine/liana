use std::fmt::Display;

use bitcoin::bip32::Fingerprint;
use iced::{
    alignment::Horizontal,
    widget::{column, row},
    Alignment, Length,
};

use crate::{
    color,
    component::{
        badge::{self, Tile},
        button::{self, EntryWidth, ListEntryAccent},
        collapse, form,
        text::{self, new::caption},
        tooltip,
    },
    icon,
    spacing::HSpacing,
    theme,
    widget::{Button, Container, Element, Row},
};

use super::text::truncate;

const COLLAPSIBLE_ENTRY_CONTENT_BOTTOM_PADDING: u16 = 10;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum EntryAccent {
    Simple,
    Warning,
    Success,
    Bitcoin,
    Testnet,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum EntryKeyKind {
    Internal,
    External,
    SafetyNet,
}

impl From<EntryKeyKind> for Tile {
    fn from(kind: EntryKeyKind) -> Self {
        match kind {
            EntryKeyKind::Internal => Tile::KeyInternal,
            EntryKeyKind::External => Tile::KeyExternal,
            EntryKeyKind::SafetyNet => Tile::KeyService,
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum EntryPathRole {
    Primary,
    Recovery,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum EntrySetKeyOwner {
    Own,
    Other,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum EntryRegisterStatus {
    Registered,
    Unregistered,
}

/// The trailing status shown on a hardware-device entry.
#[derive(Debug, Clone)]
pub enum DeviceStatus {
    None,
    AlreadyUsed,
    Fingerprint(Fingerprint),
    Selectable(Fingerprint),
    Processing,
    NotInPath,
    Unrelated,
    WrongNetwork,
    ConnectionError,
    Locked(Option<String>),
    OutdatedFirmware(String),
    Signed,
    Registered,
    Selected,
    Warning(&'static str),
}

fn device_success_mark<'a, M: 'static>(label: Option<&'static str>) -> Element<'a, M> {
    row![label.map(text::new::b5_medium), badge::success()]
        .align_y(Alignment::Center)
        .spacing(5)
        .into()
}

impl<'a, M: 'static> From<DeviceStatus> for Option<Element<'a, M>> {
    fn from(status: DeviceStatus) -> Self {
        let secondary = |label: String| -> Element<'a, M> {
            text::new::small_caption(label)
                .style(theme::text::secondary)
                .into()
        };
        match status {
            DeviceStatus::None => None,
            DeviceStatus::AlreadyUsed => Some(secondary("Already used".to_string())),
            DeviceStatus::Fingerprint(fp) => Some(secondary(format!("#{fp}"))),
            DeviceStatus::Selectable(fp) => Some(
                row![
                    text::new::small_caption(format!("#{fp}")).style(theme::text::secondary),
                    icon::chevron_right().size(16).style(theme::text::secondary)
                ]
                .spacing(6)
                .align_y(Alignment::Center)
                .into(),
            ),
            DeviceStatus::Processing => {
                Some(text::new::b5_medium("Processing, please check your device").into())
            }
            DeviceStatus::NotInPath => {
                Some(text::new::b5_medium("This signer is not part of this spending path.").into())
            }
            DeviceStatus::Unrelated => Some(
                text::new::b5_medium("This signing device is not related to this Liana wallet.")
                    .into(),
            ),
            DeviceStatus::WrongNetwork => {
                Some(text::new::b5_medium("Wrong network in the device settings").into())
            }
            DeviceStatus::ConnectionError => Some(text::new::b5_medium("Connection error").into()),
            DeviceStatus::Locked(Some(code)) => {
                Some(text::new::b5_medium(format!("Locked, check code: {code}")).into())
            }
            DeviceStatus::Locked(None) => Some(text::new::b5_medium("Locked").into()),
            DeviceStatus::OutdatedFirmware(version) => Some(
                text::new::b5_medium(format!("Install firmware version {version} or later")).into(),
            ),
            DeviceStatus::Signed => Some(device_success_mark(Some("Signed"))),
            DeviceStatus::Registered => Some(device_success_mark(Some("Registered"))),
            DeviceStatus::Selected => Some(device_success_mark(None)),
            DeviceStatus::Warning(w) => Some(
                tooltip::tooltip_custom(
                    w,
                    icon::warning_icon(),
                    iced::widget::tooltip::Position::Bottom,
                )
                .into(),
            ),
        }
    }
}

pub fn list_entry_row<'a, M: Clone + 'a>(
    tile: Option<Element<'a, M>>,
    body: impl Into<Element<'a, M>>,
    trailing: Option<Element<'a, M>>,
    accent: Option<ListEntryAccent>,
    width: EntryWidth,
    msg: Option<M>,
) -> Element<'a, M> {
    let body = Container::new(body).width(Length::Fill);
    let trailing = trailing.map(|trailing| Container::new(trailing).align_y(Alignment::Center));
    let content = row![tile, body, trailing]
        .spacing(16)
        .align_y(Alignment::Center)
        .width(Length::Fill);

    button::list_entry(content, accent, width, msg)
}

pub fn right_chevron<'a, M: 'a>() -> Element<'a, M> {
    icon::chevron_right()
        .size(18)
        .style(theme::text::secondary)
        .into()
}

/// The smaller, lighter chevron used as a breadcrumb separator.
pub fn breadcrumb_chevron<'a, M: 'a>() -> Element<'a, M> {
    icon::chevron_right()
        .size(13)
        .style(theme::text::border)
        .into()
}

/// Like [`list_entry_row`] but non-clickable and always rendered active, for an entry whose body is
/// interactive (e.g. a text input) rather than a button label.
pub fn list_entry_row_static<'a, M: Clone + 'a>(
    tile: Option<Element<'a, M>>,
    body: impl Into<Element<'a, M>>,
    trailing: Option<Element<'a, M>>,
    width: EntryWidth,
) -> Element<'a, M> {
    let body = Container::new(body).width(Length::Fill);
    let trailing = trailing.map(|trailing| Container::new(trailing).align_y(Alignment::Center));
    let content = row![tile, body, trailing]
        .spacing(16)
        .align_y(Alignment::Center)
        .width(Length::Fill);

    button::list_entry_with_enabled(content, None, width, true, None)
}

/// Expanded "paste an extended public key" entry: a paste tile, an xpub text input and a paste button,
/// laid out as a static (non-clickable) entry row.
pub fn entry_paste_xpub<'a, M: Clone + 'a>(
    value: &str,
    on_input: impl Fn(String) -> M + 'static,
    on_paste: M,
) -> Element<'a, M> {
    let form_value = form::Value {
        value: value.to_string(),
        warning: None,
        valid: true,
    };
    list_entry_row_static(
        Some(badge::tile(Tile::Paste).into()),
        form::Form::new("xpub...", &form_value, on_input).padding(10),
        Some(Button::new(icon::paste_icon()).on_press(on_paste).into()),
        EntryWidth::Standard,
    )
}

pub struct CollapsibleEntry<'a, M> {
    pub accent: Option<EntryAccent>,
    pub tile: Tile,
    pub title: &'static str,
    pub collapsed_subtitle: Option<&'static str>,
    pub expanded_subtitle: Option<&'static str>,
    pub content: Element<'a, M>,
    pub expanded: bool,
    pub on_toggle: M,
}

pub fn entry_collapsible<'a, M: Clone + 'static>(cfg: CollapsibleEntry<'a, M>) -> Element<'a, M> {
    let accent = cfg.accent.map(entry_accent);
    let entry = collapse::Collapse::new(
        collapsible_entry_header(
            cfg.tile,
            cfg.title.to_string(),
            cfg.collapsed_subtitle.map(str::to_string),
        ),
        collapsible_entry_header(
            cfg.tile,
            cfg.title.to_string(),
            cfg.expanded_subtitle.map(str::to_string),
        ),
        Container::new(cfg.content).padding(iced::Padding {
            left: button::LIST_ENTRY_PADDING[1].into(),
            right: button::LIST_ENTRY_PADDING[1].into(),
            bottom: COLLAPSIBLE_ENTRY_CONTENT_BOTTOM_PADDING.into(),
            ..iced::Padding::ZERO
        }),
    )
    .expanded(cfg.expanded)
    .on_toggle(move || cfg.on_toggle.clone())
    .style(move |theme, status| button::list_entry_style(theme, status, accent, true))
    .style_bounds()
    .padding(button::LIST_ENTRY_PADDING)
    .width(Length::Fill);

    button::list_entry_card(entry, accent, EntryWidth::Standard)
}

fn collapsible_entry_header<'a, M: Clone + 'a>(
    tile: Tile,
    title: String,
    subtitle: Option<String>,
) -> Element<'a, M> {
    let content = row![
        badge::tile_accent(tile),
        Container::new(item_body(title, subtitle)).width(Length::Fill),
    ]
    .spacing(16)
    .align_y(Alignment::Center)
    .width(Length::Fill);

    Container::new(content).width(Length::Fill).into()
}

pub fn entry_organization<'a, M: Clone + 'a>(
    title: impl Display,
    subtitle: Option<impl Display>,
    msg: Option<M>,
) -> Element<'a, M> {
    list_entry_chevron(
        Some(badge::tile(Tile::Org).into()),
        section_body(title, subtitle),
        None,
        None,
        EntryWidth::Standard,
        msg,
    )
}

/// A login account row: the account entry (tile + email + chevron) followed by a delete
/// button. While `connecting`, the email is replaced by a centered "Connecting..." label
/// and the row is inert (both buttons disabled).
pub fn account_entry<'a, M: Clone + 'a>(
    email: impl Display,
    connecting: bool,
    on_select: Option<M>,
    on_delete: Option<M>,
) -> Element<'a, M> {
    let (on_select, on_delete) = if connecting {
        (None, None)
    } else {
        (on_select, on_delete)
    };

    let body = account_body(email, connecting);

    let entry = list_entry_row(
        Some(badge::tile(Tile::Account).into()),
        body,
        Some(right_chevron()),
        None,
        EntryWidth::Deletable,
        on_select,
    );

    with_delete_button(entry, on_delete)
}

pub fn account_select_entry<'a, M: Clone + 'a>(
    email: impl Display,
    connecting: bool,
    on_select: Option<M>,
) -> Element<'a, M> {
    let on_select = if connecting { None } else { on_select };

    let body = account_body(email, connecting);

    list_entry_row(
        Some(badge::tile(Tile::Account).into()),
        body,
        Some(right_chevron()),
        None,
        EntryWidth::Standard,
        on_select,
    )
}

fn account_body<'a, M: 'a>(email: impl Display, connecting: bool) -> Element<'a, M> {
    if connecting {
        text::new::b5_medium("Connecting...")
            .width(Length::Fill)
            .align_x(Horizontal::Center)
            .into()
    } else {
        item_body(email, None::<String>)
    }
}

/// Append a delete (cross) button after a [`EntryWidth::Deletable`] entry, in the reserved slot.
fn with_delete_button<'a, M: Clone + 'a>(
    entry: Element<'a, M>,
    on_delete: Option<M>,
) -> Element<'a, M> {
    row![
        entry,
        Container::new(button::btn_remove(on_delete)).center_x(button::ENTRY_DELETE_SLOT)
    ]
    .spacing(button::ENTRY_DELETE_GAP)
    .align_y(Alignment::Center)
    .width(Length::Shrink)
    .into()
}

pub fn entry_wallet<'a, M: Clone + 'a>(
    accent: Option<EntryAccent>,
    title: impl Display,
    subtitle: Option<Element<'a, M>>,
    role_pill: Option<Element<'a, M>>,
    status_pill: Option<Element<'a, M>>,
    msg: Option<M>,
) -> Element<'a, M> {
    let title = title.to_string();
    let title = truncate(&title, 25);

    let accent = accent.map(|s| entry_accent(s));

    list_entry_chevron(
        Some(badge::tile(Tile::Wallet).into()),
        wallet_body(title, role_pill, subtitle),
        status_pill,
        accent,
        EntryWidth::Standard,
        msg,
    )
}

pub fn list_entry_chevron<'a, M: Clone + 'a>(
    tile: Option<Element<'a, M>>,
    body: impl Into<Element<'a, M>>,
    trailing: Option<Element<'a, M>>,
    accent: Option<ListEntryAccent>,
    width: EntryWidth,
    msg: Option<M>,
) -> Element<'a, M> {
    let trailing = row![trailing, right_chevron()]
        .spacing(HSpacing::ML)
        .align_y(Alignment::Center);
    let body = Container::new(body).width(Length::Fill);
    let content = row![tile, body, trailing]
        .spacing(16)
        .align_y(Alignment::Center)
        .width(Length::Fill);

    button::list_entry(content, accent, width, msg)
}

/// "(1 key)" / "({n} keys)" caption shown beside a wallet title; `None` for no keys.
pub fn key_count<'a, M: 'a>(count: usize) -> Option<Element<'a, M>> {
    let label = match count {
        0 => return None,
        1 => "(1 key)".to_string(),
        n => format!("({n} keys)"),
    };
    Some(
        text::new::caption(label)
            .style(theme::text::secondary)
            .into(),
    )
}

pub fn entry_key<'a, M: Clone + 'a>(
    kind: EntryKeyKind,
    title: impl Display,
    kind_pill: Element<'a, M>,
    signer: impl Display,
    trailing: Option<Element<'a, M>>,
    msg: Option<M>,
    on_delete: Option<M>,
) -> Element<'a, M> {
    let width = if on_delete.is_some() {
        EntryWidth::Deletable
    } else {
        EntryWidth::Standard
    };
    let entry = list_entry_row(
        Some(badge::tile(kind.into()).into()),
        key_body(title, kind_pill, signer),
        trailing,
        Some(key_accent(kind)),
        width,
        msg,
    );

    if on_delete.is_some() {
        with_delete_button(entry, on_delete)
    } else {
        entry
    }
}

#[allow(clippy::too_many_arguments)]
pub fn entry_path<'a, M: Clone + 'a>(
    role: EntryPathRole,
    title: impl Display,
    summary: impl Display,
    availability: Element<'a, M>,
    key_pills: Vec<Element<'a, M>>,
    deletable: bool,
    on_delete: Option<M>,
    msg: Option<M>,
) -> Element<'a, M> {
    // When the list is deletable every card shares the deletable width (so the primary, which has
    // no delete button, still lines up with the recovery cards); otherwise they fill the standard
    // width.
    let width = if deletable {
        EntryWidth::Deletable
    } else {
        EntryWidth::Standard
    };
    let entry = button::list_entry(
        path_body(title, summary, availability, key_pills),
        Some(path_accent(role)),
        width,
        msg,
    );

    if on_delete.is_some() {
        with_delete_button(entry, on_delete)
    } else {
        entry
    }
}

pub fn entry_set_key<'a, M: Clone + 'a>(
    kind: EntryKeyKind,
    owner: EntrySetKeyOwner,
    title: impl Display,
    subtitle: Option<impl Display>,
    trailing: Option<Element<'a, M>>,
    msg: Option<M>,
) -> Element<'a, M> {
    leaf_entry(
        kind.into(),
        title,
        subtitle,
        trailing,
        Some(set_key_accent(owner)),
        EntryWidth::Standard,
        msg,
    )
}

pub fn entry_register<'a, M: Clone + 'a>(
    status: EntryRegisterStatus,
    body: impl Into<Element<'a, M>>,
    trailing: Option<Element<'a, M>>,
    enabled: bool,
    msg: Option<M>,
) -> Element<'a, M> {
    list_entry_row_with_enabled(
        Some(badge::tile(Tile::Device).into()),
        body,
        trailing,
        Some(register_accent(status)),
        EntryWidth::Standard,
        enabled,
        msg,
    )
}

fn list_entry_row_with_enabled<'a, M: Clone + 'a>(
    tile: Option<Element<'a, M>>,
    body: impl Into<Element<'a, M>>,
    trailing: Option<Element<'a, M>>,
    accent: Option<ListEntryAccent>,
    width: EntryWidth,
    enabled: bool,
    msg: Option<M>,
) -> Element<'a, M> {
    let body = Container::new(body).width(Length::Fill);
    let trailing = trailing.map(|trailing| Container::new(trailing).align_y(Alignment::Center));
    let content = row![tile, body, trailing]
        .spacing(16)
        .align_y(Alignment::Center)
        .width(Length::Fill);

    button::list_entry_with_enabled(content, accent, width, enabled, msg)
}

pub fn entry_device_list<'a, M: Clone + 'static>(
    title: impl Display,
    subtitle: Option<impl Display>,
    trailing: DeviceStatus,
    width: EntryWidth,
    msg: Option<M>,
) -> Element<'a, M> {
    leaf_entry(
        Tile::Device,
        title,
        subtitle,
        trailing.into(),
        None,
        width,
        msg,
    )
}

/// A device-entry-styled action row (leading badge tile + body) for arbitrary actions.
pub fn entry_action<'a, M: Clone + 'a>(
    tile: Tile,
    title: impl Display,
    subtitle: Option<impl Display>,
    trailing: Option<Element<'a, M>>,
    width: EntryWidth,
    msg: Option<M>,
) -> Element<'a, M> {
    leaf_entry(tile, title, subtitle, trailing, None, width, msg)
}

pub fn entry_action_accent<'a, M: Clone + 'a>(
    accent: Option<EntryAccent>,
    tile: Tile,
    title: impl Display,
    subtitle: Option<impl Display>,
    trailing: Option<Element<'a, M>>,
    width: EntryWidth,
    msg: Option<M>,
) -> Element<'a, M> {
    leaf_entry(
        tile,
        title,
        subtitle,
        trailing,
        accent.map(entry_accent),
        width,
        msg,
    )
}

pub fn entry_no_devices<'a, M: Clone + 'a>(
    title: impl Display,
    subtitle: Option<impl Display>,
) -> Element<'a, M> {
    leaf_entry(
        Tile::DeviceMuted,
        title,
        subtitle,
        None,
        None,
        EntryWidth::Standard,
        None,
    )
}

/// "See more" button paginating an history. Shows "Fetching ..." and
/// is disabled while `processing`.
pub fn see_more<'a, M: Clone + 'a>(processing: bool, next: M) -> Element<'a, M> {
    let label = if processing {
        "Fetching ..."
    } else {
        "See more"
    };

    let button = Button::new(
        text::text(label)
            .width(Length::Fill)
            .align_x(Horizontal::Center),
    )
    .width(Length::Fill)
    .padding(15)
    .style(theme::button::transparent_border)
    .on_press_maybe((!processing).then_some(next));

    Container::new(button)
        .width(Length::Fill)
        .style(theme::card::simple)
        .into()
}

fn leaf_entry<'a, M: Clone + 'a>(
    tile: Tile,
    title: impl Display,
    subtitle: Option<impl Display>,
    trailing: Option<Element<'a, M>>,
    accent: Option<ListEntryAccent>,
    width: EntryWidth,
    msg: Option<M>,
) -> Element<'a, M> {
    let tile = if accent.is_some() {
        badge::tile_accent(tile)
    } else {
        badge::tile(tile)
    };

    list_entry_row(
        Some(tile.into()),
        item_body(title, subtitle),
        trailing,
        accent,
        width,
        msg,
    )
}

fn section_body<'a, M: 'a>(title: impl Display, subtitle: Option<impl Display>) -> Element<'a, M> {
    body(text::new::h3_semi(title), subtitle)
}

fn item_body<'a, M: 'a>(title: impl Display, subtitle: Option<impl Display>) -> Element<'a, M> {
    body(text::new::b5_medium(title), subtitle)
}

fn wallet_body<'a, M: 'a>(
    title: impl Display,
    role: Option<Element<'a, M>>,
    subtitle: Option<Element<'a, M>>,
) -> Element<'a, M> {
    let title = row![text::new::h3_semi(title), role]
        .spacing(10)
        .align_y(Alignment::Start);
    let content = column![title, subtitle].width(Length::Fill);

    Container::new(content).width(Length::Fill).into()
}

fn key_body<'a, M: 'a>(
    title: impl Display,
    kind_pill: Element<'a, M>,
    signer: impl Display,
) -> Element<'a, M> {
    let title = row![
        text::new::b5_medium(title).style(theme::text::primary),
        kind_pill
    ]
    .spacing(10)
    .align_y(Alignment::Center);
    let signer = Container::new(text::new::caption(signer).style(theme::text::tertiary))
        .padding(iced::Padding {
            top: 3.0,
            ..iced::Padding::ZERO
        })
        .width(Length::Fill);
    let content = column![title, signer].width(Length::Fill);

    Container::new(content).width(Length::Fill).into()
}

fn path_body<'a, M: 'a>(
    title: impl Display,
    summary: impl Display,
    availability: Element<'a, M>,
    key_pills: Vec<Element<'a, M>>,
) -> Element<'a, M> {
    let title_block = column![
        text::new::h3_semi(title),
        Container::new(caption(summary).style(theme::text::tertiary)).padding(iced::Padding {
            top: 2.0,
            ..iced::Padding::ZERO
        }),
    ];
    let header = row![
        Container::new(title_block).width(Length::Fill),
        availability
    ]
    .spacing(16)
    .align_y(Alignment::Start)
    .width(Length::Fill);
    let pills = Row::with_children(key_pills).spacing(9).wrap();
    column![
        header,
        Container::new(pills).padding(iced::Padding {
            top: 10.0,
            ..iced::Padding::ZERO
        })
    ]
    .width(Length::Fill)
    .into()
}

fn body<'a, M: 'a>(
    title: crate::widget::Text<'a>,
    subtitle: Option<impl Display>,
) -> Element<'a, M> {
    let subtitle: Option<Element<'a, M>> = subtitle.map(|subtitle| {
        text::new::caption(subtitle)
            .style(theme::text::secondary)
            .into()
    });
    let content = column![title, subtitle].spacing(2).width(Length::Fill);

    Container::new(content).width(Length::Fill).into()
}

fn entry_accent(status: EntryAccent) -> ListEntryAccent {
    match status {
        EntryAccent::Simple => |theme| {
            theme
                .colors
                .pills
                .simple
                .border
                .unwrap_or(theme.colors.general.accent)
        },
        EntryAccent::Warning => |theme| {
            theme
                .colors
                .pills
                .warning
                .border
                .unwrap_or(theme.colors.text.warning)
        },
        EntryAccent::Success => |theme| {
            theme
                .colors
                .pills
                .success
                .border
                .unwrap_or(theme.colors.text.success)
        },
        EntryAccent::Bitcoin => |t| t.colors.general.accent,
        EntryAccent::Testnet => |_| color::BLUE,
    }
}

fn key_accent(kind: EntryKeyKind) -> ListEntryAccent {
    match kind {
        EntryKeyKind::Internal => |theme| {
            theme
                .colors
                .pills
                .internal
                .border
                .unwrap_or(theme.colors.general.accent)
        },
        EntryKeyKind::External => |theme| {
            theme
                .colors
                .pills
                .external
                .border
                .unwrap_or(theme.colors.text.primary)
        },
        EntryKeyKind::SafetyNet => |theme| {
            theme
                .colors
                .pills
                .safety_net
                .border
                .unwrap_or(theme.colors.text.secondary)
        },
    }
}

fn path_accent(role: EntryPathRole) -> ListEntryAccent {
    match role {
        EntryPathRole::Primary => |theme| {
            theme
                .colors
                .pills
                .internal
                .border
                .unwrap_or(theme.colors.general.accent)
        },
        EntryPathRole::Recovery => |theme| {
            theme
                .colors
                .pills
                .safety_net
                .border
                .unwrap_or(theme.colors.text.secondary)
        },
    }
}

fn set_key_accent(owner: EntrySetKeyOwner) -> ListEntryAccent {
    match owner {
        EntrySetKeyOwner::Own => |theme| {
            theme
                .colors
                .pills
                .internal
                .border
                .unwrap_or(theme.colors.general.accent)
        },
        EntrySetKeyOwner::Other => |theme| {
            theme
                .colors
                .pills
                .safety_net
                .border
                .unwrap_or(theme.colors.text.secondary)
        },
    }
}

fn register_accent(status: EntryRegisterStatus) -> ListEntryAccent {
    match status {
        EntryRegisterStatus::Registered => |theme| {
            theme
                .colors
                .pills
                .success
                .border
                .unwrap_or(theme.colors.text.success)
        },
        EntryRegisterStatus::Unregistered => |theme| {
            theme
                .colors
                .pills
                .internal
                .border
                .unwrap_or(theme.colors.general.accent)
        },
    }
}

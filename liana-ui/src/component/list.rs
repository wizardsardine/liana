use std::fmt::Display;

use iced::{
    alignment::Horizontal,
    widget::{column, row},
    Alignment, Length,
};

use crate::{
    component::{
        badge::{self, Tile},
        button::{self, EntryWidth, ListEntryAccent},
        text::{self, new::caption},
    },
    icon, theme,
    widget::{Button, Container, Element, Row},
};

const PATH_DELETE_SLOT: u32 = 24;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum EntryStatus {
    Simple,
    Warning,
    Success,
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
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum DeviceStatus {
    /// No status (no trailing).
    None,
    /// The device's key is already assigned to another key in the wallet.
    AlreadyUsed,
}

impl<'a, M: 'a> From<DeviceStatus> for Option<Element<'a, M>> {
    fn from(status: DeviceStatus) -> Self {
        match status {
            DeviceStatus::None => None,
            DeviceStatus::AlreadyUsed => Some(
                text::new::small_caption("Already used")
                    .style(theme::text::secondary)
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

pub fn entry_chevron<'a, M: 'a>() -> Element<'a, M> {
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

pub fn entry_organization<'a, M: Clone + 'a>(
    title: impl Display,
    subtitle: Option<impl Display>,
    trailing: Option<Element<'a, M>>,
    msg: Option<M>,
) -> Element<'a, M> {
    list_entry_row(
        Some(badge::tile(Tile::Org).into()),
        section_body(title, subtitle),
        trailing,
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

    let body: Element<'a, M> = if connecting {
        text::new::b5_medium("Connecting...")
            .width(Length::Fill)
            .align_x(Horizontal::Center)
            .into()
    } else {
        item_body(email, None::<String>)
    };

    let entry = list_entry_row(
        Some(badge::tile(Tile::Account).into()),
        body,
        Some(entry_chevron()),
        None,
        EntryWidth::Deletable,
        on_select,
    );

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
    status: EntryStatus,
    title: impl Display,
    role: Option<Element<'a, M>>,
    subtitle: Option<Element<'a, M>>,
    trailing: Option<Element<'a, M>>,
    msg: Option<M>,
) -> Element<'a, M> {
    list_entry_row(
        Some(badge::tile(Tile::Wallet).into()),
        wallet_body(title, role, subtitle),
        trailing,
        Some(status_accent(status)),
        EntryWidth::Standard,
        msg,
    )
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
) -> Element<'a, M> {
    list_entry_row(
        Some(badge::tile(kind.into()).into()),
        key_body(title, kind_pill, signer),
        trailing,
        Some(key_accent(kind)),
        EntryWidth::Standard,
        msg,
    )
}

pub fn entry_path<'a, M: Clone + 'a>(
    role: EntryPathRole,
    title: impl Display,
    summary: impl Display,
    availability: Element<'a, M>,
    key_pills: Vec<Element<'a, M>>,
    trailing: Option<Element<'a, M>>,
    msg: Option<M>,
) -> Element<'a, M> {
    button::list_entry(
        path_body(title, summary, availability, key_pills, trailing),
        Some(path_accent(role)),
        EntryWidth::Standard,
        msg,
    )
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

pub fn entry_device_list<'a, M: Clone + 'a>(
    title: impl Display,
    subtitle: Option<impl Display>,
    trailing: impl Into<Option<Element<'a, M>>>,
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
    list_entry_row(
        Some(badge::tile(tile).into()),
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
    trailing: Option<Element<'a, M>>,
) -> Element<'a, M> {
    let title_block = column![
        text::new::h3_semi(title),
        Container::new(caption(summary).style(theme::text::tertiary)).padding(iced::Padding {
            top: 2.0,
            ..iced::Padding::ZERO
        }),
    ];
    let trailing = Container::new(trailing.unwrap_or_else(|| row![].into()))
        .width(PATH_DELETE_SLOT)
        .align_x(Horizontal::Center)
        .align_y(Alignment::Center);
    let header = row![
        Container::new(title_block).width(Length::Fill),
        availability,
        trailing
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

fn status_accent(status: EntryStatus) -> ListEntryAccent {
    match status {
        EntryStatus::Simple => |theme| {
            theme
                .colors
                .pills
                .simple
                .border
                .unwrap_or(theme.colors.general.accent)
        },
        EntryStatus::Warning => |theme| {
            theme
                .colors
                .pills
                .warning
                .border
                .unwrap_or(theme.colors.text.warning)
        },
        EntryStatus::Success => |theme| {
            theme
                .colors
                .pills
                .success
                .border
                .unwrap_or(theme.colors.text.success)
        },
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

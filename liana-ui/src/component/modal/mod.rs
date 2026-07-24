use std::fmt::Display;

use iced::{
    alignment::{Horizontal, Vertical},
    widget::{
        button::{Status, Style},
        column, row, Space,
    },
    Length, Padding,
};

use iced::widget::Container;

use bitcoin::{
    bip32::{ChildNumber, Fingerprint},
    Network,
};

use crate::{
    color,
    component::{
        badge::{self, Tile},
        button,
        form::{self, Value},
        list::{self, DeviceStatus},
        pick_list,
        text::new::{b1_bold, b4_medium, b5_bold, b5_medium, caption},
        tooltip,
    },
    icon,
    theme::{self, Theme},
};

use crate::{
    spacing::{HSpacing, VSpacing},
    widget::{Button, CheckBox, Column, Element, PickList, SpaceExt},
};

pub const BTN_W: u32 = 500;
pub const V_SPACING: VSpacing = VSpacing::S;
pub const H_SPACING: HSpacing = HSpacing::S;
const MODAL_PADDING: f32 = 20.0;
const MODAL_SPACING: VSpacing = VSpacing::M;
const TOKEN_PLACEHOLDER: &str = "aaaa-bbbb-cccc";
const MNEMONIC_PLACEHOLDER: &str = "code code code code code code code code code code code brave";

/// Modal width presets.
#[derive(Debug, Clone, Copy)]
pub enum ModalWidth {
    S = 400,
    M = 550,
    L = 650,
    XL = 800,
}

impl From<ModalWidth> for Length {
    fn from(val: ModalWidth) -> Self {
        Length::Fixed(val as u16 as f32)
    }
}

/// Keep backward compat for code referencing MODAL_WIDTH.
pub const MODAL_WIDTH: u16 = ModalWidth::L as u16;

/// Type alias for the container style function used by modal views.
pub type ContainerStyle = fn(&Theme) -> iced::widget::container::Style;

/// Standard modal wrapper: card theme + header + content with consistent
/// padding, spacing, and width.
pub fn modal_view<'a, M: 'a + Clone, C>(
    title: Option<impl Display>,
    back_message: Option<M>,
    close_message: Option<M>,
    width: ModalWidth,
    content: C,
) -> Element<'a, M>
where
    C: Into<Element<'a, M>>,
{
    modal_view_with_theme(
        title,
        back_message,
        close_message,
        width,
        content,
        theme::card::modal,
    )
}

/// Like [`modal_view`] but accepts a custom container style.
pub fn modal_view_with_theme<'a, M: 'a + Clone, C>(
    title: Option<impl Display>,
    back_message: Option<M>,
    close_message: Option<M>,
    width: ModalWidth,
    content: C,
    style: ContainerStyle,
) -> Element<'a, M>
where
    C: Into<Element<'a, M>>,
{
    let col = Column::new()
        .push(header(title, back_message, close_message))
        .push(content)
        .spacing(MODAL_SPACING)
        .padding(MODAL_PADDING)
        .width(width as u32);

    let padding = Padding {
        top: 0.0,
        right: MODAL_PADDING,
        bottom: MODAL_PADDING,
        left: MODAL_PADDING,
    };
    Container::new(col).padding(padding).style(style).into()
}

pub fn widget_style(theme: &Theme, status: Status) -> Style {
    theme::button::secondary(theme, status)
}

pub fn header<'a, M: 'a + Clone>(
    label: Option<impl Display>,
    back_message: Option<M>,
    close_message: Option<M>,
) -> Element<'a, M> {
    let back = back_message.map(button::btn_modal_previous);
    let title = label.map(b1_bold);
    let close = close_message.map(|m| button::btn_modal_close(Some(m)));
    row![back, title, Space::with_width(Length::Fill), close]
        .align_y(Vertical::Center)
        .into()
}

pub fn optional_section<'a, Message, Collapse, Fold>(
    collapsed: bool,
    title: String,
    collapse: Collapse,
    fold: Fold,
) -> Element<'a, Message>
where
    Collapse: 'static + Fn() -> Message,
    Fold: 'static + Fn() -> Message,
    Message: Clone + 'static,
{
    let icon = if collapsed {
        icon::collapsed_icon().style(theme::text::secondary)
    } else {
        icon::collapse_icon().style(theme::text::secondary)
    };

    let msg = if !collapsed { collapse() } else { fold() };

    let row = row![b5_bold(&title), icon]
        .align_y(Vertical::Center)
        .spacing(H_SPACING);

    button::btn_optional_section(row, msg).into()
}

/// Outer shell for a collapsible key/signer entry, routed through selectable
/// list entries.
pub fn collapsible_button<'a, Message, Closed, Expanded, Collapse>(
    collapsed: bool,
    closed_content: Closed,
    expanded_content: Expanded,
    collapse_message: Collapse,
) -> Element<'a, Message>
where
    Closed: Into<Element<'a, Message>>,
    Expanded: Into<Element<'a, Message>>,
    Collapse: 'static + Fn() -> Message,
    Message: Clone + 'static,
{
    if collapsed {
        button::list_entry_with_state(
            expanded_content,
            None,
            button::EntryWidth::Fill,
            true,
            false,
            None,
        )
    } else {
        button::list_entry(
            closed_content,
            None,
            button::EntryWidth::Fill,
            Some(collapse_message()),
        )
    }
}

#[allow(clippy::too_many_arguments)]
fn collapsible_input_button<'a, Message, Paste, Collapse, Input>(
    collapsed: bool,
    tile: Tile,
    label: String,
    input_placeholder: String,
    input_value: &Value<String>,
    input_message: Option<Input>,
    paste_message: Option<Paste>,
    collapse_message: Collapse,
) -> Element<'a, Message>
where
    Input: 'static + Fn(String) -> Message,
    Paste: 'static + Fn() -> Message,
    Collapse: 'static + Fn() -> Message,
    Message: Clone + 'static,
{
    let form = if let Some(input_message) = input_message {
        form::Form::new(&input_placeholder, input_value, input_message)
    } else {
        form::Form::new_disabled(&input_placeholder, input_value)
    }
    .padding(10);
    let paste = paste_message.map(|m| button::btn_paste_icon(Some(m())));

    if collapsed {
        let line = row![form, paste].spacing(H_SPACING);
        let col = column![
            row![
                caption(label).style(theme::text::primary),
                Space::fill_width()
            ],
            line
        ]
        .width(Length::Fill);
        let content = row![badge::tile(tile), col]
            .align_y(Vertical::Center)
            .spacing(list::ENTRY_H_SPACING)
            .width(Length::Fill);
        button::list_entry_with_state(content, None, button::EntryWidth::Fill, true, false, None)
    } else {
        let content = row![badge::tile(tile), caption(label)]
            .spacing(list::ENTRY_H_SPACING)
            .align_y(Vertical::Center)
            .width(Length::Fill);
        button::list_entry(
            content,
            None,
            button::EntryWidth::Fill,
            Some(collapse_message()),
        )
    }
}

/// Like [`collapsible_input_button`] but the form is gated behind a
/// disclaimer checkbox: the expanded button shows the checkbox first
/// (`!ack`), then swaps to the form once the user toggles it on (`ack`).
#[allow(clippy::too_many_arguments)]
fn acked_input_button<'a, Message, Ack, Input, Paste, Collapse>(
    collapsed: bool,
    ack: bool,
    tile: Tile,
    label: &'a str,
    disclaimer: &'a str,
    input_placeholder: &'a str,
    input_value: &Value<String>,
    ack_message: Ack,
    input_message: Input,
    paste_message: Paste,
    collapse_message: Collapse,
) -> Element<'a, Message>
where
    Ack: 'static + Fn(bool) -> Message,
    Input: 'static + Fn(String) -> Message,
    Paste: 'static + Fn() -> Message,
    Collapse: 'static + Fn() -> Message,
    Message: Clone + 'static,
{
    let form = if ack {
        form::Form::new(input_placeholder, input_value, input_message)
    } else {
        form::Form::new_disabled(input_placeholder, input_value)
    }
    .padding(10);
    let paste = Button::new(icon::paste_icon().color(color::BLACK)).on_press(paste_message());

    let expanded = {
        let line = row![form, paste].spacing(H_SPACING);
        let check_box = CheckBox::new(ack).label(disclaimer).on_toggle(ack_message);
        let label = row![caption(label).color(color::WHITE), Space::fill_width()];
        let content = if ack {
            Container::new(column![label, line])
        } else {
            Container::new(check_box)
        };
        row![badge::tile(tile), content]
            .align_y(Vertical::Center)
            .spacing(list::ENTRY_H_SPACING)
    };
    let closed = row![badge::tile(tile), caption(label)]
        .spacing(list::ENTRY_H_SPACING)
        .align_y(Vertical::Center);
    collapsible_button(collapsed, closed, expanded, collapse_message)
}

/// Where the key behind a select-key-source row comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeySourceKind {
    Device,
    HotKey,
    Xpub,
    Token,
}

impl From<KeySourceKind> for Tile {
    fn from(kind: KeySourceKind) -> Self {
        match kind {
            KeySourceKind::Device => Tile::Device,
            KeySourceKind::HotKey => Tile::KeyHot,
            KeySourceKind::Xpub => Tile::KeyExternal,
            KeySourceKind::Token => Tile::KeyService,
        }
    }
}

pub fn key_entry<'a, M: 'a + Clone>(
    kind: KeySourceKind,
    name: String,
    fingerprint: Option<String>,
    tooltip_str: Option<&'a str>,
    error: Option<String>,
    mut message: Option<String>,
    on_press: Option<M>,
) -> Element<'a, M> {
    if error.is_some() {
        message = None;
    }
    let message = message.map(caption);
    let error = error.map(|e| caption(e).color(color::ORANGE));
    let tt = tooltip_str.map(|s| tooltip(s));

    let designation = column![
        b5_bold(name),
        caption(fingerprint.unwrap_or(" - ".to_string()))
    ]
    .align_x(Horizontal::Left)
    .width(200);
    let row = row![
        badge::tile(kind.into()),
        designation,
        message,
        error,
        Space::fill_width(),
        tt
    ]
    .align_y(Vertical::Center)
    .spacing(list::ENTRY_H_SPACING)
    .width(Length::Fill);
    button::list_entry(
        row,
        None,
        button::EntryWidth::Custom(BTN_W as f32),
        on_press,
    )
}

#[derive(Debug, Clone, PartialEq)]
pub struct Account {
    pub index: ChildNumber,
    pub fingerprint: Fingerprint,
}

impl Account {
    pub fn new(index: ChildNumber, fingerprint: Fingerprint) -> Self {
        Self { index, fingerprint }
    }
}

impl Display for Account {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let index = self.index.to_string().replace('\'', "");
        write!(f, "Account #{index}")
    }
}

/// Physical device rows get the device tile, the hot signer row gets a key tile.
fn device_tile(is_device: bool) -> Tile {
    if is_device {
        Tile::Device
    } else {
        Tile::KeyInternal
    }
}

fn device_designation<'a, M: 'a>(
    kind: Option<impl Display + 'a>,
    alias: Option<impl Display + 'a>,
    fingerprint: Option<impl Display + 'a>,
) -> Column<'a, M> {
    let fg = b5_medium(
        fingerprint
            .map(|fg| fg.to_string())
            .unwrap_or_else(|| " - ".to_string()),
    );
    let fg_row = if let Some(kind) = kind {
        row![b5_bold(kind), fg].spacing(5)
    } else {
        row![fg]
    };
    if let Some(alias) = alias {
        column![b4_medium(alias), fg_row]
    } else {
        column![fg_row]
    }
    .align_x(Horizontal::Left)
}

pub fn device_entry<'a, M, F, K, A>(
    fingerprint: Option<F>,
    kind: Option<K>,
    alias: Option<A>,
    status: DeviceStatus,
    on_press: Option<M>,
) -> Element<'a, M>
where
    M: 'static + Clone,
    F: Display + 'a,
    K: Display + 'a,
    A: Display + 'a,
{
    let tile = device_tile(kind.is_some());
    let designation = device_designation(kind, alias, fingerprint);
    let row = row![
        badge::tile(tile),
        designation,
        Space::fill_width(),
        Option::<Element<'a, M>>::from(status)
    ]
    .align_y(Vertical::Center)
    .spacing(list::ENTRY_H_SPACING)
    .width(Length::Fill);
    button::list_entry(row, None, button::EntryWidth::Fill, on_press)
}

/// Derivation-account picker: a dropdown over accounts 0..10 for the given device.
pub fn account_pick_list<'a, Message: Clone + 'a>(
    fingerprint: Fingerprint,
    selected: Option<ChildNumber>,
    on_select: impl Fn(Account) -> Message + 'a,
) -> PickList<'a, Account, Vec<Account>, Account, Message> {
    let accounts: Vec<Account> = (0..10)
        .map(|i| {
            Account::new(
                ChildNumber::from_hardened_idx(i).expect("hardcoded"),
                fingerprint,
            )
        })
        .collect();
    let selected = Account::new(
        selected.unwrap_or(ChildNumber::from_hardened_idx(0).expect("hardcoded")),
        fingerprint,
    );
    pick_list::pick_list(accounts, Some(selected), on_select)
}

pub fn account_device_entry<'a, M, K, A>(
    fingerprint: Fingerprint,
    kind: Option<K>,
    alias: Option<A>,
    selected: Option<ChildNumber>,
    on_press: Option<M>,
) -> Element<'a, M>
where
    M: 'static + Clone + From<(Fingerprint, ChildNumber)>,
    K: Display + 'a,
    A: Display + 'a,
{
    let picker = account_pick_list(fingerprint, selected, |a: Account| {
        (a.fingerprint, a.index).into()
    });
    let tile = device_tile(kind.is_some());
    let designation = device_designation(kind, alias, Some(format!("#{fingerprint}")));
    let row = row![badge::tile(tile), designation, Space::fill_width(), picker]
        .align_y(Vertical::Center)
        .spacing(list::ENTRY_H_SPACING)
        .width(Length::Fill);
    button::list_entry(row, None, button::EntryWidth::Fill, on_press)
}

/// Row entry for an expected key in a registration-style flow.
pub fn registration_key_entry<'a, Message, M>(
    fingerprint: String,
    kind: Option<String>,
    alias: Option<String>,
    entry_status: list::EntryRegisterStatus,
    status: Option<String>,
    on_press: Option<M>,
) -> Element<'a, Message>
where
    M: 'static + Fn() -> Message,
    Message: Clone + 'static,
{
    let msg = on_press.map(|f| f());
    let title = alias.unwrap_or_else(|| kind.unwrap_or_else(|| fingerprint.clone()));
    let status: Option<Element<'a, Message>> = status.map(|status| b5_medium(status).into());
    let title = b5_medium(title);
    let fingerprint = caption(fingerprint).style(theme::text::secondary);
    let body = column![title, fingerprint, status]
        .spacing(2)
        .width(Length::Fill);

    list::entry_register(entry_status, body, None, msg.is_some(), msg)
}

fn button_entry<'a, Message, M>(
    tile: Tile,
    label: &'a str,
    tooltip_str: Option<&'static str>,
    error: Option<String>,
    on_press: Option<M>,
) -> Element<'a, Message>
where
    M: 'static + Fn() -> Message,
    Message: Clone + 'static,
{
    let error = error.map(|e| row![caption(e).color(color::ORANGE), Space::fill_width()]);

    let tt = tooltip_str.map(|s| tooltip(s));

    let row = row![badge::tile(tile), caption(label), Space::fill_width(), tt]
        .spacing(list::ENTRY_H_SPACING)
        .align_y(Vertical::Center);

    let col = column![row, error].width(Length::Fill);

    let msg = on_press.map(|f| f());
    button::list_entry(col, None, button::EntryWidth::Fill, msg)
}

/// Entry loading an extended public key from a file.
pub fn import_xpub_entry<'a, Message, M>(
    error: Option<String>,
    on_press: Option<M>,
) -> Element<'a, Message>
where
    M: 'static + Fn() -> Message,
    Message: Clone + 'static,
{
    button_entry(
        Tile::Import,
        "Import extended public key file",
        None,
        error,
        on_press,
    )
}

/// Entry generating a key stored on this computer.
pub fn generate_hot_key_entry<'a, Message, M>(on_press: Option<M>) -> Element<'a, Message>
where
    M: 'static + Fn() -> Message,
    Message: Clone + 'static,
{
    button_entry(
        Tile::KeyHot,
        "Generate hot key stored on this computer",
        Some("We recommend to use this option only for test purposes"),
        None,
        on_press,
    )
}

/// Collapsible entry pasting an extended public key.
pub fn paste_xpub_entry<'a, Message, Paste, Collapse, Input>(
    collapsed: bool,
    network: Network,
    input_value: &Value<String>,
    input_message: Option<Input>,
    paste_message: Option<Paste>,
    collapse_message: Collapse,
) -> Element<'a, Message>
where
    Input: 'static + Fn(String) -> Message,
    Paste: 'static + Fn() -> Message,
    Collapse: 'static + Fn() -> Message,
    Message: Clone + 'static,
{
    collapsible_input_button(
        collapsed,
        Tile::Paste,
        "Paste an extended public key".to_string(),
        example_xpub(network),
        input_value,
        input_message,
        paste_message,
        collapse_message,
    )
}

/// Collapsible entry entering a Safety Net token.
pub fn safety_net_token_entry<'a, Message, Paste, Collapse, Input>(
    collapsed: bool,
    input_value: &Value<String>,
    input_message: Option<Input>,
    paste_message: Option<Paste>,
    collapse_message: Collapse,
) -> Element<'a, Message>
where
    Input: 'static + Fn(String) -> Message,
    Paste: 'static + Fn() -> Message,
    Collapse: 'static + Fn() -> Message,
    Message: Clone + 'static,
{
    token_entry(
        "Enter a Safety Net token",
        collapsed,
        input_value,
        input_message,
        paste_message,
        collapse_message,
    )
}

/// Collapsible entry entering a Cosigner token.
pub fn cosigner_token_entry<'a, Message, Paste, Collapse, Input>(
    collapsed: bool,
    input_value: &Value<String>,
    input_message: Option<Input>,
    paste_message: Option<Paste>,
    collapse_message: Collapse,
) -> Element<'a, Message>
where
    Input: 'static + Fn(String) -> Message,
    Paste: 'static + Fn() -> Message,
    Collapse: 'static + Fn() -> Message,
    Message: Clone + 'static,
{
    token_entry(
        "Enter a Cosigner token",
        collapsed,
        input_value,
        input_message,
        paste_message,
        collapse_message,
    )
}

fn token_entry<'a, Message, Paste, Collapse, Input>(
    label: &'static str,
    collapsed: bool,
    input_value: &Value<String>,
    input_message: Option<Input>,
    paste_message: Option<Paste>,
    collapse_message: Collapse,
) -> Element<'a, Message>
where
    Input: 'static + Fn(String) -> Message,
    Paste: 'static + Fn() -> Message,
    Collapse: 'static + Fn() -> Message,
    Message: Clone + 'static,
{
    collapsible_input_button(
        collapsed,
        Tile::EnterToken,
        label.to_string(),
        TOKEN_PLACEHOLDER.to_string(),
        input_value,
        input_message,
        paste_message,
        collapse_message,
    )
}

/// Collapsible entry typing a mnemonic, gated behind a disclaimer checkbox.
#[allow(clippy::too_many_arguments)]
pub fn enter_mnemonic_entry<'a, Message, Ack, Input, Paste, Collapse>(
    collapsed: bool,
    ack: bool,
    input_value: &Value<String>,
    ack_message: Ack,
    input_message: Input,
    paste_message: Paste,
    collapse_message: Collapse,
) -> Element<'a, Message>
where
    Ack: 'static + Fn(bool) -> Message,
    Input: 'static + Fn(String) -> Message,
    Paste: 'static + Fn() -> Message,
    Collapse: 'static + Fn() -> Message,
    Message: Clone + 'static,
{
    acked_input_button(
        collapsed,
        ack,
        Tile::Mnemonic,
        "UNSAFE: Enter mnemonic of one of the keys",
        " This option is not secure. I understand that entering a mnemonic on a computer may result in theft of my funds.",
        MNEMONIC_PLACEHOLDER,
        input_value,
        ack_message,
        input_message,
        paste_message,
        collapse_message,
    )
}

fn example_xpub(network: Network) -> String {
    format!("[aabbccdd/42'/0']{}pub6DAkq8LWw91WGgUGnkR5Sbzjev5JCsXaTVZQ9MwsPV4BkNFKygtJ8GHodfDVx1udR723nT7JASqGPpKvz7zQ25pUTW6zVEBdiWoaC4aUqik",
        if network == Network::Bitcoin { "x" } else { "t" }
    )
}

pub fn modal_no_devices_placeholder<'a, M: 'a>() -> Element<'a, M> {
    column![
        icon::usb_icon().size(100),
        caption("No hardware device detected. Connect a device and unlock it."),
    ]
    .align_x(Horizontal::Center)
    .spacing(20)
    .into()
}

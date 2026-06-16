pub mod legacy;

use std::fmt::Display;

use iced::{
    alignment::{Horizontal, Vertical},
    widget::{
        button::{Status, Style},
        column, row,
        tooltip::Position,
        Space,
    },
    Length, Padding,
};

use iced::widget::Container;

use bitcoin::bip32::{ChildNumber, Fingerprint};

use crate::{
    color,
    component::{
        badge, button,
        form::{self, Value},
        pick_list, text,
        text::new::{b4_medium, b5_medium},
        tooltip,
    },
    icon,
    theme::{self, Theme},
};

use crate::widget::{Button, CheckBox, Column, ColumnExt, Element, Row, RowExt, SpaceExt, Text};

pub const BTN_W: u32 = 500;
pub const V_SPACING: u32 = 10;
pub const H_SPACING: u32 = 5;
const MODAL_PADDING: f32 = 20.0;
const MODAL_SPACING: u32 = 15;

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
    let back = back_message
        .map(|m| button::transparent(Some(icon::arrow_back().size(25)), "").on_press(m));
    let title = label.map(text::h3);
    let close = close_message.map(|m| {
        Button::new(icon::cross_icon().size(40))
            .padding(0)
            .style(theme::button::transparent)
            .on_press(m)
    });
    Row::new()
        .push_maybe(back)
        .push_maybe(title)
        .push(Space::with_width(Length::Fill))
        .push_maybe(close)
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

    let row = Row::new()
        .push(text::p1_bold(&title))
        .push(icon)
        .align_y(Vertical::Center)
        .spacing(H_SPACING);

    Button::new(row)
        .style(theme::button::transparent_border)
        .on_press(msg)
        .into()
}

/// Outer shell for a collapsible key/signer entry, routed through the
/// `button::device*` helpers.
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
        button::device_with_height_clickable(expanded_content, None, None, false)
    } else {
        button::device(closed_content, Some(collapse_message()))
    }
}

#[allow(clippy::too_many_arguments)]
pub fn collapsible_input_button<'a, Message, Paste, Collapse, Input>(
    collapsed: bool,
    icon: Option<Text<'static>>,
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
    let paste = paste_message.map(|m| Button::new(icon::paste_icon()).on_press(m()));

    if collapsed {
        let icon = icon.map(|i| i.style(theme::text::primary));
        let line = Row::new().push(form).push_maybe(paste).spacing(V_SPACING);
        let col = Column::new()
            .push(row![
                text::p1_regular(label).style(theme::text::primary),
                Space::with_width(Length::Fill)
            ])
            .push(line)
            .width(Length::Fill);
        let content = Row::new()
            .push_maybe(icon)
            .push(col)
            .align_y(Vertical::Center)
            .spacing(V_SPACING)
            .width(Length::Fill);
        button::device_with_height_clickable(content, None, None, false)
    } else {
        let content = Row::new()
            .push_maybe(icon.as_ref().map(|_| Space::with_width(H_SPACING)))
            .push_maybe(icon)
            .push(Space::with_width(H_SPACING))
            .push(text::p1_regular(label))
            .spacing(V_SPACING)
            .align_y(Vertical::Center);
        button::device(content, Some(collapse_message()))
    }
}

/// Like [`collapsible_input_button`] but the form is gated behind a
/// disclaimer checkbox: the expanded button shows the checkbox first
/// (`!ack`), then swaps to the form once the user toggles it on (`ack`).
#[allow(clippy::too_many_arguments)]
pub fn acked_input_button<'a, Message, Ack, Input, Paste, Collapse, I>(
    collapsed: bool,
    ack: bool,
    icon: I,
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
    I: Fn() -> Text<'static>,
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
        let line = row![form, paste].spacing(V_SPACING);
        let check_box = CheckBox::new(ack).label(disclaimer).on_toggle(ack_message);
        let label = row![
            text::p1_regular(label).color(color::WHITE),
            Space::fill_width()
        ];
        let content = if ack {
            Container::new(column![label, line])
        } else {
            Container::new(check_box)
        };
        row![icon(), content]
            .align_y(Vertical::Center)
            .spacing(V_SPACING)
    };
    let closed = row![icon(), text::p1_regular(label)]
        .spacing(V_SPACING)
        .align_y(Vertical::Center);
    collapsible_button(collapsed, closed, expanded, collapse_message)
}

pub fn key_entry<'a, M: 'a + Clone>(
    icon: Option<Text<'a>>,
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
    let message = message.map(text::p2_regular);
    let error = error.map(|e| text::p1_regular(e).color(color::ORANGE));
    let tt = tooltip_str.map(|s| tooltip(s));

    let designation = column![
        text::p1_bold(name),
        text::p1_regular(fingerprint.unwrap_or(" - ".to_string()))
    ]
    .align_x(Horizontal::Left)
    .width(200);
    let row = Row::new()
        .push_maybe(icon.as_ref().map(|_| Space::with_width(H_SPACING)))
        .push_maybe(icon)
        .push(Space::with_width(H_SPACING))
        .push(designation)
        .push_maybe(message)
        .push_maybe(error)
        .push(Space::with_width(Length::Fill))
        .push_maybe(tt)
        .align_y(Vertical::Center)
        .spacing(V_SPACING);
    button::device(row, on_press)
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

pub enum DeviceMark {
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
}

impl Display for DeviceMark {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeviceMark::Processing => write!(f, "Processing, please check your device"),
            DeviceMark::NotInPath => write!(f, "This signer is not part of this spending path."),
            DeviceMark::Unrelated => {
                write!(
                    f,
                    "This signing device is not related to this Liana wallet."
                )
            }
            DeviceMark::WrongNetwork => write!(f, "Wrong network in the device settings"),
            DeviceMark::ConnectionError => write!(f, "Connection error"),
            DeviceMark::Locked(Some(code)) => write!(f, "Locked, check code: {code}"),
            DeviceMark::Locked(None) => write!(f, "Locked"),
            DeviceMark::OutdatedFirmware(version) => {
                write!(f, "Install firmware version {version} or later")
            }
            DeviceMark::Signed => write!(f, "Signed"),
            DeviceMark::Registered => write!(f, "Registered"),
            DeviceMark::Selected => Ok(()),
        }
    }
}

impl DeviceMark {
    pub fn element<'a, M: 'static>(&self) -> Element<'a, M> {
        match self {
            DeviceMark::Signed | DeviceMark::Registered => success_mark(Some(self.to_string())),
            DeviceMark::Selected => success_mark(None),
            _ => b5_medium(self.to_string()).into(),
        }
    }

    pub fn warning(&self) -> Option<&'static str> {
        match self {
            DeviceMark::WrongNetwork => Some(
                "The wrong bitcoin application is open or the device was initialized with the wrong network",
            ),
            DeviceMark::OutdatedFirmware(_) => Some("Please upgrade firmware"),
            DeviceMark::ConnectionError => {
                Some("Make sure your device is unlocked and a supported Bitcoin application is opened.")
            }
            _ => None,
        }
    }
}

fn device_icon(is_device: bool) -> Text<'static> {
    if is_device {
        icon::usb_drive_icon()
    } else {
        icon::round_key_icon()
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
        row![b5_medium(kind), fg].spacing(5)
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

fn success_mark<'a, M: 'static>(label: Option<String>) -> Element<'a, M> {
    row![label.map(b5_medium), badge::success()]
        .align_y(Vertical::Center)
        .spacing(H_SPACING)
        .into()
}

pub fn device_entry<'a, M, F, K, A>(
    fingerprint: Option<F>,
    kind: Option<K>,
    alias: Option<A>,
    mark: Option<DeviceMark>,
    warning: Option<&'static str>,
    on_press: Option<M>,
) -> Element<'a, M>
where
    M: 'static + Clone,
    F: Display + 'a,
    K: Display + 'a,
    A: Display + 'a,
{
    let icon = device_icon(kind.is_some());
    let warning = warning.or_else(|| mark.as_ref().and_then(DeviceMark::warning));
    let mark: Option<Element<'a, M>> = mark.map(|m| m.element());
    let warning =
        warning.map(|w| tooltip::tooltip_custom(w, icon::warning_icon(), Position::Bottom));
    let designation = device_designation(kind, alias, fingerprint);
    let row = row![icon, designation, Space::fill_width(), mark, warning]
        .align_y(Vertical::Center)
        .spacing(H_SPACING);
    button::device(row, on_press)
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
    let picker = pick_list::pick_list(accounts, Some(selected), |a: Account| {
        (a.fingerprint, a.index).into()
    });
    let icon = device_icon(kind.is_some());
    let designation = device_designation(kind, alias, Some(format!("#{fingerprint}")));
    let row = row![icon, designation, Space::fill_width(), picker]
        .align_y(Vertical::Center)
        .spacing(H_SPACING);
    button::device(row, on_press)
}

/// Row entry for an expected key in a registration-style flow.
pub fn registration_key_entry<'a, Message, M>(
    fingerprint: String,
    kind: Option<String>,
    alias: Option<String>,
    status: Option<String>,
    on_press: Option<M>,
) -> Element<'a, Message>
where
    M: 'static + Fn() -> Message,
    Message: Clone + 'static,
{
    let icon = if kind.is_some() {
        icon::usb_drive_icon()
    } else {
        icon::round_key_icon()
    };
    let fg = text::p1_medium(fingerprint);
    let fg_row = if let Some(k) = kind {
        row![text::p1_bold(k), fg].spacing(5)
    } else {
        row![fg]
    };
    let designation = if let Some(alias) = alias {
        column![text::h5_medium(alias), fg_row]
    } else {
        column![fg_row]
    }
    .align_x(Horizontal::Left);

    let status = status.map(text::p1_medium);
    let row = Row::new()
        .push(Space::with_width(H_SPACING))
        .push(icon)
        .push(Space::with_width(H_SPACING))
        .push(designation)
        .push(Space::fill_width())
        .push_maybe(status)
        .push(Space::fill_width())
        .align_y(Vertical::Center)
        .spacing(V_SPACING);
    let msg = on_press.map(|f| f());
    button::device(row, msg)
}

pub fn button_entry<'a, Message, M>(
    icon: Option<Text<'static>>,
    label: &'a str,
    tooltip_str: Option<&'static str>,
    error: Option<String>,
    on_press: Option<M>,
) -> Element<'a, Message>
where
    M: 'static + Fn() -> Message,
    Message: Clone + 'static,
{
    let error = error.map(|e| {
        row![
            text::p1_regular(e).color(color::ORANGE),
            Space::with_width(Length::Fill)
        ]
    });

    let tt = tooltip_str.map(|s| tooltip(s));

    let row = Row::new()
        .push_maybe(icon.as_ref().map(|_| Space::with_width(H_SPACING)))
        .push_maybe(icon)
        .push(Space::with_width(H_SPACING))
        .push(text::p1_regular(label))
        .push(Space::fill_width())
        .push_maybe(tt)
        .spacing(V_SPACING)
        .align_y(Vertical::Center);

    let col = Column::new()
        .push(row)
        .push_maybe(error)
        .width(Length::Fill);

    let msg = on_press.map(|f| f());
    button::device(col, msg)
}

pub fn modal_no_devices_placeholder<'a, M: 'a>() -> Element<'a, M> {
    Column::new()
        .push(icon::usb_icon().size(100))
        .push(text::p1_regular("Plug in a hardware device ..."))
        .align_x(Horizontal::Center)
        .spacing(20)
        .into()
}

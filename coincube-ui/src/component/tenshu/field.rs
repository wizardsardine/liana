//! Labelled text fields (`.inp` in Send and Receive, `.ctl` in the Activity
//! filters) and the search field built on them.
//!
//! A field is a label, the input and optional lines under it: an error in the
//! bad tone and a notice (each with its glyph on a regular field), then the
//! `.sub` line with a hint on the left and a trailing hint on the right, such
//! as Send's "≈ $0.00 · Available 25,053 SATS". Pressing the label emits the owner's focus
//! intent for the input, so the label is as good a target as the input. A
//! field without `on_input` is disabled: drawn muted, read-only, and skipped by
//! Tab and whole-tree focus like a disabled action.
//!
//! A search field is compact and, once it has a value and an `on_clear`
//! intent, shows a Clear action inside its right edge (no artboard search
//! draws a magnifier; [`TextField::magnifier`] adds one). Escape in a
//! non-empty search field emits the same intent without leaving the field.
//! Clearing by the Clear action, by pointer, Enter or Space, also requests
//! focus for the input, since Clear itself disappears with the value.
//!
//! Fields hold no state beyond Iced's own cursor and selection; the caller owns
//! the value and validation.

use std::borrow::Cow;

use iced::advanced::text::Shaping;
use iced::advanced::widget::Id;
use iced::keyboard::{self, key};
use iced::widget::text::LineHeight;
use iced::widget::text_input::{Icon, Side};
use iced::{alignment, Alignment, Font, Length, Padding};

use super::scope::{Inert, KeyScope};
use crate::font;
use crate::icon;
use crate::theme::tenshu::button::{self as button_style, ActionKind, ActionShape};
use crate::theme::tenshu::control as style;
use crate::theme::tenshu::{radius, size, text as text_style, type_size, Tone};
use crate::theme::Theme;
use crate::widget::action::Action;
use crate::widget::{Column, Container, Element, Renderer, Row, RowExt};

/// Field height, radius and text size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FieldSize {
    /// 44px, 10px radius, 15px text: Send and Receive (`.inp`).
    #[default]
    Regular,
    /// 36px, 8px radius, 13.5px text: filters and setting rows (`.ctl`).
    Compact,
}

impl FieldSize {
    fn height(self) -> f32 {
        match self {
            FieldSize::Regular => size::FIELD_HEIGHT,
            FieldSize::Compact => size::FIELD_COMPACT_HEIGHT,
        }
    }

    fn radius(self) -> f32 {
        match self {
            FieldSize::Regular => radius::MENU,
            FieldSize::Compact => radius::CONTROL,
        }
    }

    fn text_size(self, mono: bool) -> f32 {
        match (self, mono) {
            (FieldSize::Regular, false) => type_size::FIELD,
            (FieldSize::Regular, true) => type_size::BODY,
            (FieldSize::Compact, false) => type_size::FIELD_COMPACT,
            (FieldSize::Compact, true) => type_size::MONO,
        }
    }

    fn padding_x(self) -> f32 {
        match self {
            FieldSize::Regular => 14.0,
            FieldSize::Compact => 10.0,
        }
    }
}

/// Every field draws its text on a fixed 20px line so the height is exact.
const LINE: f32 = 20.0;
/// Room kept clear at the right for a suffix (`.inw .inp`).
const SUFFIX_ROOM: f32 = 64.0;
/// The Clear action inside a search field.
const CLEAR_SIZE: f32 = 24.0;
const SEARCH_CODE_POINT: char = '\u{F52A}';
const BOOTSTRAP_ICONS: Font = Font::with_name("bootstrap-icons");

/// A labelled text field. Build it, then convert it into an [`Element`].
pub struct TextField<'a, M> {
    id: Id,
    label: Cow<'a, str>,
    value: &'a str,
    placeholder: Cow<'a, str>,
    size: FieldSize,
    focus_intent: fn(Id) -> M,
    on_input: Option<Box<dyn Fn(String) -> M + 'a>>,
    on_submit: Option<M>,
    on_paste: Option<Box<dyn Fn(String) -> M + 'a>>,
    suffix: Option<Cow<'a, str>>,
    suffix_segment_width: Option<f32>,
    hint: Option<Cow<'a, str>>,
    trailing_hint: Option<Cow<'a, str>>,
    notice: Option<Cow<'a, str>>,
    error: Option<Cow<'a, str>>,
    label_hidden: bool,
    magnifier: bool,
    mono: bool,
    secure: bool,
    width: Length,
    clear: Option<(Id, M)>,
    accessory: Option<Element<'a, M>>,
}

/// A text field with a visible `label` over the input showing `value`.
/// `focus_intent` must apply Iced's whole-tree focus operation for the given
/// `Id`; the label emits it for the input's `id`.
pub fn text_field<'a, M: Clone + 'static>(
    id: impl Into<Id>,
    label: impl Into<Cow<'a, str>>,
    value: &'a str,
    focus_intent: fn(Id) -> M,
) -> TextField<'a, M> {
    TextField {
        id: id.into(),
        label: label.into(),
        value,
        placeholder: Cow::Borrowed(""),
        size: FieldSize::Regular,
        focus_intent,
        on_input: None,
        on_submit: None,
        on_paste: None,
        suffix: None,
        suffix_segment_width: None,
        hint: None,
        trailing_hint: None,
        notice: None,
        error: None,
        label_hidden: false,
        magnifier: false,
        mono: false,
        secure: false,
        width: Length::Fill,
        clear: None,
        accessory: None,
    }
}

/// A compact search field. Add [`TextField::on_clear`] for the Clear action
/// and Escape.
pub fn search_field<'a, M: Clone + 'static>(
    id: impl Into<Id>,
    label: impl Into<Cow<'a, str>>,
    value: &'a str,
    focus_intent: fn(Id) -> M,
) -> TextField<'a, M> {
    TextField {
        size: FieldSize::Compact,
        ..text_field(id, label, value, focus_intent)
    }
}

impl<'a, M: Clone + 'static> TextField<'a, M> {
    pub fn placeholder(mut self, placeholder: impl Into<Cow<'a, str>>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    pub fn size(mut self, size: FieldSize) -> Self {
        self.size = size;
        self
    }

    pub fn on_input(mut self, on_input: impl Fn(String) -> M + 'a) -> Self {
        self.on_input = Some(Box::new(on_input));
        self
    }

    /// `None` disables the field: read-only, muted and out of the Tab order.
    pub fn on_input_maybe(mut self, on_input: Option<impl Fn(String) -> M + 'a>) -> Self {
        self.on_input = on_input.map(|f| Box::new(f) as Box<dyn Fn(String) -> M + 'a>);
        self
    }

    /// Emitted when Enter is pressed in the focused field.
    pub fn on_submit(mut self, message: M) -> Self {
        self.on_submit = Some(message);
        self
    }

    pub fn on_paste(mut self, on_paste: impl Fn(String) -> M + 'a) -> Self {
        self.on_paste = Some(Box::new(on_paste));
        self
    }

    /// A unit drawn inside the right edge, such as "sats".
    pub fn suffix(mut self, suffix: impl Into<Cow<'a, str>>) -> Self {
        self.suffix = Some(suffix.into());
        self
    }

    /// A fixed domain or unit in its own trailing segment. Reserves `width`
    /// for the segment so long input values scroll clear of the suffix.
    pub fn segmented_suffix(mut self, suffix: impl Into<Cow<'a, str>>, width: f32) -> Self {
        self.suffix = Some(suffix.into());
        self.suffix_segment_width = Some(width);
        self
    }

    /// A muted line under the field.
    pub fn hint(mut self, hint: impl Into<Cow<'a, str>>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// A muted line at the right of the hint line (`.sub`), such as
    /// "Available 25,053 SATS".
    pub fn trailing_hint(mut self, hint: impl Into<Cow<'a, str>>) -> Self {
        self.trailing_hint = Some(hint.into());
        self
    }

    /// A muted message with an info glyph, such as "This invoice sets its own
    /// amount." (`.msg.muted`).
    pub fn notice(mut self, notice: impl Into<Cow<'a, str>>) -> Self {
        self.notice = Some(notice.into());
        self
    }

    /// Keep the label for the field's API but do not draw it, as Buy/Sell and
    /// P2P do (`.srl`) where a heading directly above names the search. Use
    /// only with such a heading.
    pub fn hide_label(mut self) -> Self {
        self.label_hidden = true;
        self
    }

    /// A leading magnifier inside the field.
    pub fn magnifier(mut self) -> Self {
        self.magnifier = true;
        self
    }

    /// Marks the field invalid: a bad border and this message under it.
    pub fn error(mut self, error: impl Into<Cow<'a, str>>) -> Self {
        self.error = Some(error.into());
        self
    }

    pub fn error_maybe(mut self, error: Option<impl Into<Cow<'a, str>>>) -> Self {
        self.error = error.map(Into::into);
        self
    }

    /// IBM Plex Mono, for addresses, invoices and identifiers.
    pub fn mono(mut self) -> Self {
        self.mono = true;
        self
    }

    /// Mask the value, for PINs and passwords.
    pub fn secure(mut self, secure: bool) -> Self {
        self.secure = secure;
        self
    }

    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }

    /// A control at the right end of the label line, such as the sats / BTC
    /// unit switch over Send's Amount field. It is its own focus stop, before
    /// the input.
    pub fn label_accessory(mut self, accessory: impl Into<Element<'a, M>>) -> Self {
        self.accessory = Some(accessory.into());
        self
    }

    /// The Clear action (with its own stable `id`) and Escape both emit
    /// `message` while the field has a value. Only meaningful on an enabled
    /// field.
    pub fn on_clear(mut self, id: impl Into<Id>, message: M) -> Self {
        self.clear = Some((id.into(), message));
        self
    }
}

fn label_text<'a>(
    content: Cow<'a, str>,
    size: FieldSize,
    enabled: bool,
) -> crate::widget::Text<'a> {
    let (text_size, weight) = match size {
        FieldSize::Regular => (type_size::BODY, font::SEMIBOLD),
        FieldSize::Compact => (type_size::CAPTION, font::MEDIUM),
    };
    let tone = match (size, enabled) {
        (_, false) | (FieldSize::Compact, true) => Tone::Muted,
        (FieldSize::Regular, true) => Tone::Neutral,
    };
    iced::widget::text(content)
        .shaping(Shaping::Advanced)
        .font(weight)
        .size(text_size)
        .line_height(LineHeight::Relative(type_size::LINE_HEIGHT))
        .style(move |theme| text_style::tone(theme, tone))
}

/// A line under the field: 13px on a regular field (`.msg`, `.sub`), 12px on a
/// compact one (`.ferr`).
fn note<'a>(content: Cow<'a, str>, tone: Tone, size: FieldSize) -> crate::widget::Text<'a> {
    let text_size = match size {
        FieldSize::Regular => type_size::CHIP,
        FieldSize::Compact => type_size::CAPTION,
    };
    iced::widget::text(content)
        .shaping(Shaping::Advanced)
        .font(font::REGULAR)
        .size(text_size)
        .line_height(LineHeight::Relative(type_size::LINE_HEIGHT))
        .style(move |theme| text_style::tone(theme, tone))
}

/// A message (`.msg`): on a regular field its 16px glyph leads the text.
fn message<'a, M: 'a>(
    content: Cow<'a, str>,
    tone: Tone,
    glyph: crate::widget::Text<'a>,
    size: FieldSize,
) -> Element<'a, M> {
    let text = note(content, tone, size);
    match size {
        FieldSize::Regular => Row::new()
            .spacing(6)
            .align_y(Alignment::Start)
            .push(
                glyph
                    .size(size::ICON_SMALL)
                    .width(Length::Fixed(size::ICON_SMALL))
                    .style(move |theme| text_style::tone(theme, tone)),
            )
            .push(text)
            .into(),
        FieldSize::Compact => text.into(),
    }
}

fn field_control<'a, M: Clone + 'static>(
    input: iced::widget::TextInput<'a, M, Theme, Renderer>,
    overlay: Option<Element<'a, M>>,
    enabled: bool,
    id: &Id,
    focus_intent: fn(Id) -> M,
    clear: Option<(Id, M)>,
) -> KeyScope<'a, M> {
    let input: Element<'a, M> = if enabled {
        input.into()
    } else {
        Inert::new(input).into()
    };
    // The scope and the input (the stack's first child) keep their places
    // whatever the value, so clearing the field (by Escape or the Clear
    // action) keeps Iced's focus and cursor state in the input. With no
    // overlay the stack holds the input alone: an empty second child still
    // opens a draw layer, and on a 2x display that empty layer clipped
    // every later overlay (App preferences over a Cube) to this page.
    let control = iced::widget::Stack::with_children(std::iter::once(input).chain(overlay))
        .width(Length::Fill);
    let input_id = id.clone();
    KeyScope::new(
        control,
        Box::new(move |key, modifiers, focused| {
            let (clear_id, message) = clear.as_ref()?;
            let keyboard::Key::Named(named) = key else {
                return None;
            };
            match named {
                // Escape in the input clears it and keeps focus there.
                key::Named::Escape if focused == Some(&input_id) => Some(vec![message.clone()]),
                // Enter or Space on Clear: the Clear action leaves with the
                // value, so hand focus back to the input first, as a
                // pointer press on Clear does.
                key::Named::Enter | key::Named::Space
                    if focused == Some(clear_id) && !modifiers.command() && !modifiers.alt() =>
                {
                    Some(vec![focus_intent(input_id.clone()), message.clone()])
                }
                _ => None,
            }
        }),
    )
}

fn visible_heading<'a, M: 'a>(
    mut column: Column<'a, M>,
    label: Element<'a, M>,
    accessory: Option<Element<'a, M>>,
) -> Column<'a, M> {
    let label_row: Element<'a, M> = match accessory {
        Some(accessory) => Row::new()
            .align_y(Alignment::Center)
            .push(Container::new(label).width(Length::Fill))
            .push(accessory)
            .into(),
        None => label,
    };
    column = column.push(label_row);
    column
}

fn field_messages<'a, M: 'a>(
    mut column: Column<'a, M>,
    error: Option<Cow<'a, str>>,
    notice: Option<Cow<'a, str>>,
    hint: Option<Cow<'a, str>>,
    trailing_hint: Option<Cow<'a, str>>,
    size: FieldSize,
) -> Column<'a, M> {
    let mut lines: Vec<Element<'a, M>> = Vec::new();
    if let Some(error) = error {
        lines.push(message(
            error,
            Tone::Bad,
            icon::exclamation_circle_icon(),
            size,
        ));
    }
    if let Some(notice) = notice {
        lines.push(message(notice, Tone::Muted, icon::tooltip_icon(), size));
    }
    if hint.is_some() || trailing_hint.is_some() {
        lines.push(
            Row::new()
                .spacing(12)
                .push(
                    Container::new(
                        hint.map(|hint| note(hint, Tone::Muted, size))
                            .unwrap_or_else(|| note(Cow::Borrowed(""), Tone::Muted, size)),
                    )
                    .width(Length::Fill),
                )
                .push_maybe(trailing_hint.map(|hint| note(hint, Tone::Muted, size)))
                .into(),
        );
    }
    if !lines.is_empty() {
        column = column.push(
            lines
                .into_iter()
                .fold(Column::new().spacing(4), |column, line| column.push(line)),
        );
    }
    column
}

fn install_input_handlers<'a, M: Clone + 'static>(
    mut input: iced::widget::TextInput<'a, M, Theme, Renderer>,
    magnifier: bool,
    on_input: Option<Box<dyn Fn(String) -> M + 'a>>,
    on_paste: Option<Box<dyn Fn(String) -> M + 'a>>,
    on_submit: Option<M>,
    enabled: bool,
) -> iced::widget::TextInput<'a, M, Theme, Renderer> {
    if magnifier {
        input = input.icon(Icon {
            font: BOOTSTRAP_ICONS,
            code_point: SEARCH_CODE_POINT,
            size: Some(size::ICON_SMALL.into()),
            spacing: 8.0,
            side: Side::Left,
        });
    }
    if let Some(on_input) = on_input {
        input = input.on_input(on_input);
    }
    if let Some(on_paste) = on_paste.filter(|_| enabled) {
        input = input.on_paste(on_paste);
    }
    if let Some(on_submit) = on_submit.filter(|_| enabled) {
        input = input.on_submit(on_submit);
    }

    input
}

impl<'a, M: Clone + 'static> From<TextField<'a, M>> for Element<'a, M> {
    fn from(field: TextField<'a, M>) -> Self {
        let TextField {
            id,
            label,
            value,
            placeholder,
            size,
            focus_intent,
            on_input,
            on_submit,
            on_paste,
            suffix,
            suffix_segment_width,
            hint,
            trailing_hint,
            notice,
            error,
            label_hidden,
            magnifier,
            mono,
            secure,
            width,
            clear,
            accessory,
        } = field;
        let enabled = on_input.is_some();
        let invalid = error.is_some();
        let clear = clear.filter(|_| enabled && !value.is_empty());
        let field_radius = if suffix_segment_width.is_some() {
            radius::CONTROL
        } else {
            size.radius()
        };

        let right = match (&suffix, &clear) {
            (Some(_), _) => {
                suffix_segment_width.map_or(SUFFIX_ROOM, |width| width + size.padding_x())
            }
            (None, Some(_)) => size.padding_x() + CLEAR_SIZE + 4.0,
            (None, None) => size.padding_x(),
        };
        let vertical = (size.height() - LINE) / 2.0;
        let input = iced::widget::TextInput::<'a, M, Theme, Renderer>::new(&placeholder, value)
            .id(id.clone())
            .secure(secure)
            .font(if mono {
                font::MONO_REGULAR
            } else {
                font::REGULAR
            })
            .size(size.text_size(mono))
            .line_height(LineHeight::Absolute(LINE.into()))
            .padding(Padding {
                top: vertical,
                right,
                bottom: vertical,
                left: size.padding_x(),
            })
            .width(Length::Fill)
            .style(move |theme, status| {
                let mut appearance = style::field(theme, status, field_radius, invalid);
                if suffix_segment_width.is_some() {
                    appearance.background = iced::Background::Color(theme.colors.shell.panel);
                }
                appearance
            });
        let input =
            install_input_handlers(input, magnifier, on_input, on_paste, on_submit, enabled);

        let overlay: Option<Element<'a, M>> = match (suffix, &clear, suffix_segment_width) {
            (Some(suffix), _, Some(segment_width)) => {
                let segment = Row::new()
                    .push(
                        Container::new(iced::widget::Space::new())
                            .width(1)
                            .height(Length::Fill)
                            .style(style::divider),
                    )
                    .push(
                        Container::new(note(suffix, Tone::Muted, size))
                            .padding([0, 14])
                            .center_y(Length::Fill),
                    );
                Some(
                    Container::new(
                        Container::new(segment)
                            .width(segment_width)
                            .height(Length::Fill)
                            .style(move |theme: &Theme| iced::widget::container::Style {
                                background: Some(iced::Background::Color(
                                    theme.colors.shell.background,
                                )),
                                border: iced::Border {
                                    radius: iced::border::Radius {
                                        top_right: field_radius - 2.0,
                                        bottom_right: field_radius - 2.0,
                                        ..Default::default()
                                    },
                                    ..Default::default()
                                },
                                ..Default::default()
                            }),
                    )
                    .padding(2)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .align_x(alignment::Horizontal::Right)
                    .into(),
                )
            }
            (Some(suffix), _, None) => Some(
                Container::new(
                    iced::widget::text(suffix)
                        .shaping(Shaping::Advanced)
                        .font(font::MEDIUM)
                        .size(type_size::CHIP)
                        .style(move |theme| text_style::tone(theme, Tone::Muted)),
                )
                .width(Length::Fill)
                .height(Length::Fill)
                .padding([0.0, size.padding_x()])
                .align_x(alignment::Horizontal::Right)
                .align_y(alignment::Vertical::Center)
                .into(),
            ),
            (None, Some((clear_id, message)), _) => {
                let glyph = Container::new(icon::cross_icon().size(size::ICON_SMALL))
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .align_x(alignment::Horizontal::Center)
                    .align_y(alignment::Vertical::Center);
                // Pressing Clear hands focus back to the input it empties;
                // the Clear action itself leaves with the value.
                let input_id = id.clone();
                let action = Action::new(clear_id.clone(), glyph, move |_| focus_intent(input_id))
                    .style(|theme, status| {
                        button_style::action(
                            theme,
                            status,
                            ActionKind::Secondary,
                            ActionShape::Icon,
                        )
                    })
                    .width(Length::Fixed(CLEAR_SIZE))
                    .height(Length::Fixed(CLEAR_SIZE))
                    .padding(0.0)
                    .focus_radius(radius::ROW)
                    .on_press(message.clone());
                Some(
                    Container::new(action)
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .padding([0.0, size.padding_x() - 4.0])
                        .align_x(alignment::Horizontal::Right)
                        .align_y(alignment::Vertical::Center)
                        .into(),
                )
            }
            (None, None, _) => None,
        };

        let control = field_control(input, overlay, enabled, &id, focus_intent, clear);

        let label = label_text(label, size, enabled);
        let label: Element<'a, M> = if enabled {
            iced::widget::MouseArea::new(label)
                .on_press(focus_intent(id))
                .into()
        } else {
            label.into()
        };

        let mut column = Column::new().spacing(6).width(width);
        if label_hidden {
            // Hiding the label must not hide an independently focusable
            // accessory. Keep it above the input and aligned to the right.
            if let Some(accessory) = accessory {
                column = column.push(
                    Row::new()
                        .push(iced::widget::Space::new().width(Length::Fill))
                        .push(accessory),
                );
            }
        } else {
            column = visible_heading(column, label, accessory);
        }
        column = column.push(control);
        column = field_messages(column, error, notice, hint, trailing_hint, size);
        column.into()
    }
}

//! One current value out of a few, drawn in the artboards' five forms:
//!
//! - [`segmented`] (`.seg`): Bitcoin / Stablecoin in Send and Receive, BTC /
//!   Sats in Settings, the theme in App preferences.
//! - [`choice_chips`] (`.sb`): the Settings scope selector.
//! - [`tabs`] (`.tab`): Settings section tabs, an accent underline under the
//!   current one.
//! - [`nav_list`] (`.ni`): the App preferences section list.
//! - [`radio_list`] (`.radio`): Settings radio rows with a description.
//!
//! Each segment is an [`Action`] with its own stable ID, so it keeps Action's
//! keyboard contract: a Tab stop, one activation per Enter/Space press, the
//! focus ring and the owner's focus intent on pointer press. While a segment
//! has focus, the arrow keys move the selection to the previous or next
//! enabled segment (wrapping), requesting focus for it as well. A disabled
//! segment, or every segment of a group without an intent, is inert and
//! leaves the Tab order.
//!
//! The group holds no state. Pressing a segment, including the selected one,
//! emits `on_select` with its value; owners treat selection as idempotent.

use std::borrow::Cow;

use iced::advanced::text::Shaping;
use iced::advanced::widget::Id;
use iced::keyboard::{self, key};
use iced::{alignment, Alignment, Color, Length};

use super::scope::KeyScope;
use crate::font;
use crate::theme::palette::ShellPalette;
use crate::theme::tenshu::control as style;
use crate::theme::tenshu::{self as tokens, radius, size, text as text_style, type_size, Tone};
use crate::theme::Theme;
use crate::widget::action::Action;
use crate::widget::{Column, Container, Element, Row, Text};

/// Segment height and frame radius.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SegmentSize {
    /// 32px segments in a 10px frame: the Send and Receive master selectors.
    #[default]
    Regular,
    /// 28px segments in an 8px frame: choices inside a setting row.
    Compact,
    /// 26px segments in a 10px frame: inline unit switches such as sats / BTC.
    Small,
}

impl SegmentSize {
    fn height(self) -> f32 {
        match self {
            SegmentSize::Regular => size::SEGMENT_HEIGHT,
            SegmentSize::Compact => size::SEGMENT_COMPACT_HEIGHT,
            SegmentSize::Small => size::SEGMENT_SMALL_HEIGHT,
        }
    }

    fn frame_radius(self) -> f32 {
        match self {
            SegmentSize::Compact => radius::CONTROL,
            SegmentSize::Regular | SegmentSize::Small => radius::MENU,
        }
    }

    fn segment_radius(self) -> f32 {
        match self {
            SegmentSize::Compact => radius::ROW,
            SegmentSize::Regular | SegmentSize::Small => radius::CONTROL,
        }
    }

    fn text_size(self) -> f32 {
        match self {
            SegmentSize::Small => type_size::CAPTION,
            SegmentSize::Regular | SegmentSize::Compact => type_size::CHIP,
        }
    }

    fn padding_x(self) -> f32 {
        match self {
            SegmentSize::Small => 10.0,
            SegmentSize::Regular | SegmentSize::Compact => 12.0,
        }
    }
}

/// One option of a choice.
pub struct Segment<'a, T> {
    id: Id,
    value: T,
    label: Cow<'a, str>,
    icon: Option<Text<'a>>,
    description: Option<Cow<'a, str>>,
    enabled: bool,
}

/// An option with a stable ID, the value it selects and its visible label.
pub fn segment<'a, T>(
    id: impl Into<Id>,
    value: T,
    label: impl Into<Cow<'a, str>>,
) -> Segment<'a, T> {
    Segment {
        id: id.into(),
        value,
        label: label.into(),
        icon: None,
        description: None,
        enabled: true,
    }
}

impl<'a, T> Segment<'a, T> {
    /// A leading glyph: 16px in segments and chips, 18px in the nav list.
    pub fn icon(mut self, icon: Text<'a>) -> Self {
        self.icon = Some(icon);
        self
    }

    /// A muted line under the label, shown by [`radio_list`] only.
    pub fn description(mut self, description: impl Into<Cow<'a, str>>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// An unavailable option: drawn faded, never selected by pointer, keys or
    /// arrows. Explain why in text next to the group.
    pub fn disabled(mut self) -> Self {
        self.enabled = false;
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
}

/// Which artboard form a choice takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Look {
    Segmented(SegmentSize),
    Chips,
    Tabs,
    Nav,
    Radio,
}

/// A choice. Build it, then convert it into an [`Element`].
pub struct Segmented<'a, T, M> {
    segments: Vec<Segment<'a, T>>,
    selected: Option<T>,
    look: Look,
    outlined: bool,
    focus_intent: fn(Id) -> M,
    on_select: Option<Box<dyn Fn(T) -> M + 'a>>,
}

fn choice<'a, T, M>(
    segments: impl IntoIterator<Item = Segment<'a, T>>,
    selected: Option<T>,
    focus_intent: fn(Id) -> M,
    look: Look,
) -> Segmented<'a, T, M> {
    Segmented {
        segments: segments.into_iter().collect(),
        selected,
        look,
        outlined: true,
        focus_intent,
        on_select: None,
    }
}

/// A segmented choice over `segments`, showing `selected` (or nothing). The
/// `focus_intent` must apply Iced's whole-tree focus operation for the given
/// `Id`, as [`Action::new`] requires.
pub fn segmented<'a, T, M>(
    segments: impl IntoIterator<Item = Segment<'a, T>>,
    selected: Option<T>,
    focus_intent: fn(Id) -> M,
) -> Segmented<'a, T, M>
where
    T: Copy + PartialEq + 'a,
    M: Clone + 'static,
{
    choice(
        segments,
        selected,
        focus_intent,
        Look::Segmented(SegmentSize::default()),
    )
}

/// Wrapping 32px chips (`.sb`); the selected chip takes the selected fill, an
/// accent outline and an accent-ink glyph.
pub fn choice_chips<'a, T, M>(
    segments: impl IntoIterator<Item = Segment<'a, T>>,
    selected: Option<T>,
    focus_intent: fn(Id) -> M,
) -> Segmented<'a, T, M>
where
    T: Copy + PartialEq + 'a,
    M: Clone + 'static,
{
    choice(segments, selected, focus_intent, Look::Chips)
}

/// Section tabs over a 1px rule (`.tabs`, `.tab`); the current tab is ink,
/// semibold and underlined in the accent.
pub fn tabs<'a, T, M>(
    segments: impl IntoIterator<Item = Segment<'a, T>>,
    selected: Option<T>,
    focus_intent: fn(Id) -> M,
) -> Segmented<'a, T, M>
where
    T: Copy + PartialEq + 'a,
    M: Clone + 'static,
{
    choice(segments, selected, focus_intent, Look::Tabs)
}

/// A vertical section list (`.nav`, `.ni`): 40px rows; the current one takes
/// the selected fill, semibold text and an accent glyph.
pub fn nav_list<'a, T, M>(
    segments: impl IntoIterator<Item = Segment<'a, T>>,
    selected: Option<T>,
    focus_intent: fn(Id) -> M,
) -> Segmented<'a, T, M>
where
    T: Copy + PartialEq + 'a,
    M: Clone + 'static,
{
    choice(segments, selected, focus_intent, Look::Nav)
}

/// Radio rows (`.radio`): a 16px radio, the label and its description, each
/// row above a 1px rule.
pub fn radio_list<'a, T, M>(
    segments: impl IntoIterator<Item = Segment<'a, T>>,
    selected: Option<T>,
    focus_intent: fn(Id) -> M,
) -> Segmented<'a, T, M>
where
    T: Copy + PartialEq + 'a,
    M: Clone + 'static,
{
    choice(segments, selected, focus_intent, Look::Radio)
}

impl<'a, T, M> Segmented<'a, T, M>
where
    T: Copy + PartialEq + 'a,
    M: Clone + 'static,
{
    /// The segment size of a [`segmented`] choice; other forms ignore it.
    pub fn size(mut self, size: SegmentSize) -> Self {
        if let Look::Segmented(_) = self.look {
            self.look = Look::Segmented(size);
        }
        self
    }

    /// Drop the selected segment's accent outline: the Settings artboard
    /// marks the selection with the selected fill and semibold ink only.
    pub fn plain(mut self) -> Self {
        self.outlined = false;
        self
    }

    pub fn on_select(mut self, on_select: impl Fn(T) -> M + 'a) -> Self {
        self.on_select = Some(Box::new(on_select));
        self
    }

    /// `None` makes every option inert.
    pub fn on_select_maybe(mut self, on_select: Option<impl Fn(T) -> M + 'a>) -> Self {
        self.on_select = on_select.map(|f| Box::new(f) as Box<dyn Fn(T) -> M + 'a>);
        self
    }
}

fn label_text<'a>(label: Cow<'a, str>, size: f32, selected: bool, rest: iced::Font) -> Text<'a> {
    iced::widget::text(label)
        .shaping(Shaping::Advanced)
        .font(if selected { font::SEMIBOLD } else { rest })
        .size(size)
}

/// A glyph in the colour `color` picks, faded with the control when disabled.
fn glyph<'a>(
    icon: Text<'a>,
    glyph_size: f32,
    color: fn(&ShellPalette) -> Color,
    enabled: bool,
) -> Text<'a> {
    icon.size(glyph_size)
        .width(Length::Fixed(glyph_size))
        .style(move |theme: &Theme| {
            let color = color(&theme.colors.shell);
            iced::widget::text::Style {
                color: Some(if enabled {
                    color
                } else {
                    tokens::faded(color, tokens::DISABLED_ALPHA)
                }),
            }
        })
}

fn muted(shell: &ShellPalette) -> Color {
    shell.muted
}

fn ink(shell: &ShellPalette) -> Color {
    shell.ink
}

fn accent_ink(shell: &ShellPalette) -> Color {
    shell.accent_ink
}

fn accent(shell: &ShellPalette) -> Color {
    shell.accent
}

fn with_icon<'a, M: 'a>(icon: Option<Text<'a>>, label: Text<'a>, gap: f32) -> Element<'a, M> {
    match icon {
        Some(icon) => Row::new()
            .spacing(gap)
            .align_y(Alignment::Center)
            .push(icon)
            .push(label)
            .into(),
        None => label.into(),
    }
}

fn segment_item<'a, T: Copy + PartialEq + 'a, M: Clone + 'static>(
    segment: Segment<'a, T>,
    selected: Option<T>,
    look: Look,
    outlined: bool,
    focus_intent: fn(Id) -> M,
    on_select: &Option<Box<dyn Fn(T) -> M + 'a>>,
) -> Element<'a, M> {
    let Segment {
        id,
        value,
        label,
        icon,
        description,
        enabled,
    } = segment;
    let is_selected = selected == Some(value);
    // A group without an on_select handler is inert even when its
    // individual segments were not explicitly disabled.
    let interactive = enabled && on_select.is_some();
    let on_press = on_select
        .as_ref()
        .filter(|_| interactive)
        .map(|on_select| on_select(value));
    let action = match look {
        Look::Segmented(size) => {
            let content = framed_segment_content(label, icon, size, is_selected, interactive);
            let segment_radius = size.segment_radius();
            Action::new(id, content, focus_intent)
                .style(move |theme, status| {
                    let mut style = style::segment(theme, status, is_selected, segment_radius);
                    if !outlined {
                        style.border.width = 0.0;
                    }
                    style
                })
                .height(Length::Fixed(size.height()))
                .padding([0.0, size.padding_x()])
                .focus_radius(segment_radius)
        }
        Look::Chips => {
            let label = label_text(label, type_size::CHIP, is_selected, font::REGULAR);
            let color = if is_selected { accent_ink } else { muted };
            let icon = icon.map(|icon| glyph(icon, size::ICON_SMALL, color, interactive));
            let content = Container::new(with_icon(icon, label, 6.0))
                .height(Length::Fill)
                .align_y(alignment::Vertical::Center);
            Action::new(id, content, focus_intent)
                .style(move |theme, status| style::choice_chip(theme, status, is_selected))
                .height(Length::Fixed(size::CHIP_HEIGHT))
                .padding([0.0, 12.0])
                .focus_radius(radius::CHIP)
        }
        Look::Tabs => {
            let label = label_text(label, type_size::BODY, is_selected, font::REGULAR).line_height(
                iced::widget::text::LineHeight::Relative(type_size::LINE_HEIGHT),
            );
            // `.subnav button`: an optional glyph 8px before the
            // label, ink on the current tab and muted otherwise.
            let color = if is_selected { ink } else { muted };
            let icon = icon.map(|icon| glyph(icon, size::ICON_SMALL, color, interactive));
            let label = with_icon(icon, label, 8.0);
            let underline = Container::new(iced::widget::Space::new())
                .width(Length::Fill)
                .height(Length::Fixed(2.0))
                .style(move |theme| style::tab_underline(theme, is_selected));
            // The underline fills the tab's width; the width is set
            // after the pushes because pushing a Fill child widens
            // a column to Fill.
            let content = Column::new()
                .spacing(8)
                .push(Container::new(label).padding([0.0, 12.0]))
                .push(underline)
                .width(Length::Shrink);
            Action::new(id, content, focus_intent)
                .style(move |theme, status| style::tab(theme, status, is_selected))
                .padding(iced::Padding::default().top(10.0))
                .focus_radius(radius::ROW)
        }
        Look::Nav => {
            let label = label_text(label, type_size::BODY, is_selected, font::REGULAR);
            let color = if is_selected { accent } else { muted };
            let icon = icon.map(|icon| glyph(icon, size::ICON, color, interactive));
            let content = Container::new(with_icon(icon, label, 10.0))
                .width(Length::Fill)
                .height(Length::Fill)
                .align_y(alignment::Vertical::Center);
            Action::new(id, content, focus_intent)
                .style(move |theme, status| style::nav_item(theme, status, is_selected))
                .width(Length::Fill)
                .height(Length::Fixed(size::NAV_ITEM_HEIGHT))
                .padding([0.0, 12.0])
                .focus_radius(radius::CONTROL)
        }
        Look::Radio => {
            let mark = Container::new(
                Container::new(iced::widget::Space::new())
                    .width(Length::Fixed(size::RADIO_DOT))
                    .height(Length::Fixed(size::RADIO_DOT))
                    .style(move |theme| style::radio_dot(theme, is_selected, interactive)),
            )
            .width(Length::Fixed(size::RADIO))
            .height(Length::Fixed(size::RADIO))
            .align_x(alignment::Horizontal::Center)
            .align_y(alignment::Vertical::Center)
            .style(move |theme| style::radio_ring(theme, is_selected, interactive));
            let text = radio_segment_text(label, description);
            let content = Row::new()
                .spacing(10)
                .align_y(Alignment::Start)
                .width(Length::Fill)
                .push(
                    Container::new(mark)
                        .width(Length::Fixed(20.0))
                        .padding(iced::Padding::default().top(3.0)),
                )
                .push(text);
            Action::new(id, content, focus_intent)
                .style(style::radio_row)
                .width(Length::Fill)
                .padding([10.0, 0.0])
                .focus_radius(radius::ROW)
        }
    };
    action.on_press_maybe(on_press).into()
}

fn framed_segment_content<'a, M: 'a>(
    label: Cow<'a, str>,
    icon: Option<Text<'a>>,
    size: SegmentSize,
    is_selected: bool,
    interactive: bool,
) -> Container<'a, M> {
    let label = label_text(label, size.text_size(), is_selected, font::MEDIUM);
    let icon = icon.map(|icon| {
        if interactive {
            icon.size(size::ICON_SMALL)
        } else {
            glyph(
                icon,
                size::ICON_SMALL,
                if is_selected { ink } else { muted },
                false,
            )
        }
    });
    Container::new(with_icon(icon, label, 6.0))
        .height(Length::Fill)
        .align_x(alignment::Horizontal::Center)
        .align_y(alignment::Vertical::Center)
}

fn radio_segment_text<'a, M: 'a>(
    label: Cow<'a, str>,
    description: Option<Cow<'a, str>>,
) -> Column<'a, M> {
    let mut text = Column::new().spacing(2).width(Length::Fill).push(
        label_text(label, type_size::BODY, false, font::MEDIUM).line_height(
            iced::widget::text::LineHeight::Relative(type_size::LINE_HEIGHT),
        ),
    );
    if let Some(description) = description {
        text = text.push(
            iced::widget::text(description)
                .shaping(Shaping::Advanced)
                .size(type_size::CAPTION)
                .line_height(iced::widget::text::LineHeight::Relative(
                    type_size::LINE_HEIGHT,
                ))
                .style(move |theme| text_style::tone(theme, Tone::Muted)),
        );
    }
    text
}

impl<'a, T, M> From<Segmented<'a, T, M>> for Element<'a, M>
where
    T: Copy + PartialEq + 'a,
    M: Clone + 'static,
{
    fn from(group: Segmented<'a, T, M>) -> Self {
        let Segmented {
            segments,
            selected,
            look,
            outlined,
            focus_intent,
            on_select,
        } = group;

        // (id, message) per enabled option, in visual order, for arrow keys.
        let targets: Vec<(Id, M)> = match on_select.as_ref() {
            Some(on_select) => segments
                .iter()
                .filter(|segment| segment.enabled)
                .map(|segment| (segment.id.clone(), on_select(segment.value)))
                .collect(),
            None => Vec::new(),
        };

        let items: Vec<Element<'a, M>> = segments
            .into_iter()
            .map(|segment| {
                segment_item(segment, selected, look, outlined, focus_intent, &on_select)
            })
            .collect();

        let content: Element<'a, M> = match look {
            Look::Segmented(size) => {
                let row = items.into_iter().fold(
                    Row::new().spacing(2).align_y(Alignment::Center),
                    |row, item| row.push(item),
                );
                let frame_radius = size.frame_radius();
                Container::new(row)
                    .padding(2)
                    .style(move |theme| style::segmented_frame(theme, frame_radius))
                    .into()
            }
            Look::Chips => items
                .into_iter()
                .fold(
                    Row::new().spacing(6).align_y(Alignment::Center),
                    |row, item| row.push(item),
                )
                .wrap()
                .vertical_spacing(6)
                .into(),
            Look::Tabs => Column::new()
                .width(Length::Fill)
                .push(
                    items
                        .into_iter()
                        .fold(Row::new().spacing(4), |row, item| row.push(item))
                        .wrap(),
                )
                .push(rule())
                .into(),
            Look::Nav => items
                .into_iter()
                .fold(
                    Column::new().spacing(6).width(Length::Fill),
                    |column, item| column.push(item),
                )
                .into(),
            Look::Radio => items
                .into_iter()
                .fold(Column::new().width(Length::Fill), |column, item| {
                    column.push(rule()).push(item)
                })
                .into(),
        };

        KeyScope::new(
            content,
            Box::new(move |key, modifiers, focused| {
                if modifiers.command() || modifiers.alt() || modifiers.shift() {
                    return None;
                }
                let step: isize = match key {
                    keyboard::Key::Named(key::Named::ArrowLeft | key::Named::ArrowUp) => -1,
                    keyboard::Key::Named(key::Named::ArrowRight | key::Named::ArrowDown) => 1,
                    _ => return None,
                };
                let current = targets.iter().position(|(id, _)| Some(id) == focused)?;
                let count = targets.len() as isize;
                let next = (current as isize + step).rem_euclid(count) as usize;
                let (id, message) = targets[next].clone();
                Some(vec![focus_intent(id), message])
            }),
        )
        .into()
    }
}

fn rule<'a, M: 'a>() -> Element<'a, M> {
    Container::new(iced::widget::Space::new())
        .width(Length::Fill)
        .height(Length::Fixed(1.0))
        .style(style::divider)
        .into()
}

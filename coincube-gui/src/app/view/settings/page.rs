//! The Settings artboard's page parts: titled groups (`.grp`), ruled row
//! lists (`.rows`, `.row`), status lines (`.st`), the lead block (`.lead`),
//! notes (`.note`) and the saved line (`.saved`). Every Settings tab composes
//! these, so the tabs share one rhythm.

use coincube_ui::locale::copy as localized_copy;
use std::borrow::Cow;

use coincube_ui::component::tenshu::{self, ActionButton, ActionKind, Tone};
use coincube_ui::icon;
use coincube_ui::theme::{self, tenshu::control};
use coincube_ui::widget::{Column, Container, Element, Row, Text};
use iced::widget::text::Wrapping;
use iced::widget::Space;
use iced::{Alignment, Length};

use crate::app::view::message::Message;

/// Space above each group (`.grp { margin-top: 32px }`).
pub const GROUP_GAP: f32 = 32.0;

/// A 1px rule in the line colour.
pub fn rule<'a, M: 'a>() -> Element<'a, M> {
    Container::new(Space::new())
        .width(Length::Fill)
        .height(Length::Fixed(1.0))
        .style(control::divider)
        .into()
}

/// A 16px semibold group heading (`.grp h3`).
pub fn heading<'a>(title: impl Into<Cow<'a, str>>) -> Text<'a> {
    tenshu::title(title.into()).size(16)
}

/// A muted 13px line (`.ds`, `.intro`).
pub fn muted<'a>(content: impl Into<Cow<'a, str>>) -> Text<'a> {
    tenshu::caption(content.into()).size(13)
}

/// A compact illustrated step shared by backup authentication and Recovery Kit.
/// The supporting image has a fixed slot so long instructions wrap beside it.
pub fn illustrated_flow<'a, M: 'a>(
    title: &'a str,
    detail: &'a str,
    art: iced::widget::image::Handle,
    slot: coincube_ui::illustration::Slot,
) -> Column<'a, M> {
    Column::new()
        .width(Length::Fill)
        .max_width(640)
        .spacing(24)
        .push(Space::new().height(8))
        .push(
            Row::new()
                .width(Length::Fill)
                .spacing(24)
                .align_y(Alignment::Center)
                .push(
                    Column::new()
                        .width(Length::Fill)
                        .spacing(6)
                        .push(tenshu::heading(title))
                        .push(tenshu::body(detail).style(theme::tenshu::text::muted)),
                )
                .push(coincube_ui::illustration::image(
                    Some(slot),
                    art,
                    Length::Fixed(88.0),
                    Length::Fixed(88.0),
                    iced::ContentFit::Contain,
                )),
        )
}

/// A group: heading, an optional intro line and the content, 32px below what
/// precedes it.
pub fn group<'a, M: 'a>(
    title: impl Into<Cow<'a, str>>,
    intro: Option<Cow<'a, str>>,
    content: impl Into<Element<'a, M>>,
) -> Element<'a, M> {
    let mut column = Column::new()
        .width(Length::Fill)
        .push(Space::new().height(Length::Fixed(GROUP_GAP)))
        .push(heading(title));
    if let Some(intro) = intro {
        column = column
            .push(Space::new().height(Length::Fixed(4.0)))
            .push(muted(intro));
    }
    column
        .push(Space::new().height(Length::Fixed(12.0)))
        .push(content)
        .into()
}

/// A group whose heading carries one action on its right (`.gtop .meta`):
/// a link that belongs to the whole group, so its rows keep only their
/// status on the right.
pub fn group_with_action<'a, M: 'a>(
    title: impl Into<Cow<'a, str>>,
    action: impl Into<Element<'a, M>>,
    content: impl Into<Element<'a, M>>,
) -> Element<'a, M> {
    Column::new()
        .width(Length::Fill)
        .push(Space::new().height(Length::Fixed(GROUP_GAP)))
        .push(
            Row::new()
                .align_y(Alignment::Center)
                .push(heading(title).width(Length::Fill))
                .push(action),
        )
        .push(Space::new().height(Length::Fixed(12.0)))
        .push(content)
        .into()
}

/// Ruled rows (`.rows`): a rule above the first row; every row draws the rule
/// below itself.
pub fn rows<'a, M: 'a>(items: impl IntoIterator<Item = Element<'a, M>>) -> Element<'a, M> {
    items
        .into_iter()
        .fold(Column::new().width(Length::Fill).push(rule()), |c, item| {
            c.push(item)
        })
        .into()
}

/// One `.row`: label column (label, inline accessory, description lines) and
/// right-aligned controls, 56px tall at least, ruled below.
pub struct PageRow<'a, M> {
    label: Element<'a, M>,
    accessory: Option<Element<'a, M>>,
    lines: Vec<Element<'a, M>>,
    leading: Option<Element<'a, M>>,
    controls: Vec<Element<'a, M>>,
    top: bool,
}

pub fn row<'a, M: 'a>(label: impl Into<Cow<'a, str>>) -> PageRow<'a, M> {
    PageRow {
        label: tenshu::label(label.into()).into(),
        accessory: None,
        lines: Vec::new(),
        leading: None,
        controls: Vec::new(),
        top: false,
    }
}

impl<'a, M: 'a> PageRow<'a, M> {
    /// A status (`.st`) or scope tag (`.sc`) beside the label.
    pub fn accessory(mut self, accessory: impl Into<Element<'a, M>>) -> Self {
        self.accessory = Some(accessory.into());
        self
    }

    /// The device-scope tag (`.sc`).
    pub fn scope_tag(self, tag: &'a str) -> Self {
        self.accessory(scope_tag(tag))
    }

    pub fn description(mut self, description: impl Into<Cow<'a, str>>) -> Self {
        self.lines
            .push(Container::new(muted(description)).max_width(560).into());
        self
    }

    /// Any further element under the label (a mono value, a link, pills).
    pub fn line(mut self, line: impl Into<Element<'a, M>>) -> Self {
        self.lines.push(line.into());
        self
    }

    /// Content before the label column, such as the avatar tile.
    pub fn leading(mut self, leading: impl Into<Element<'a, M>>) -> Self {
        self.leading = Some(leading.into());
        self
    }

    pub fn control(mut self, control: impl Into<Element<'a, M>>) -> Self {
        self.controls.push(control.into());
        self
    }

    pub fn control_maybe(self, control: Option<impl Into<Element<'a, M>>>) -> Self {
        match control {
            Some(control) => self.control(control),
            None => self,
        }
    }

    /// Align the controls with the first line (`.row.atop`).
    pub fn top(mut self) -> Self {
        self.top = true;
        self
    }
}

impl<'a, M: 'a> From<PageRow<'a, M>> for Element<'a, M> {
    fn from(row: PageRow<'a, M>) -> Self {
        let PageRow {
            label,
            accessory,
            lines,
            leading,
            controls,
            top,
        } = row;
        let mut heading = Row::new().spacing(8).align_y(Alignment::Center).push(label);
        if let Some(accessory) = accessory {
            heading = heading.push(accessory);
        }
        let text = lines.into_iter().fold(
            Column::new().spacing(2).width(Length::Fill).push(heading),
            |c, line| c.push(line),
        );
        let left: Element<'a, M> = match leading {
            Some(leading) => Row::new()
                .spacing(14)
                .align_y(Alignment::Center)
                .width(Length::Fill)
                .push(leading)
                .push(text)
                .into(),
            None => text.into(),
        };
        let align = if top {
            Alignment::Start
        } else {
            Alignment::Center
        };
        let mut body = Row::new()
            .spacing(32)
            .align_y(align)
            .width(Length::Fill)
            .push(left);
        if !controls.is_empty() {
            body = body.push(
                controls
                    .into_iter()
                    .fold(Row::new().spacing(8).align_y(Alignment::Center), |r, c| {
                        r.push(c)
                    }),
            );
        }
        Column::new()
            .width(Length::Fill)
            .push(
                Container::new(body)
                    .padding([12, 0])
                    .width(Length::Fill)
                    .height(Length::Shrink)
                    .center_y(Length::Shrink)
                    .style(|_: &theme::Theme| Default::default()),
            )
            .push(Space::new().height(Length::Fixed(0.0)))
            .push(rule())
            .into()
    }
}

/// The scope tag (`.sc`): 18px, muted 12px text in a 1px line, 4px radius.
pub fn scope_tag<'a, M: 'a>(tag: &'a str) -> Element<'a, M> {
    Container::new(tenshu::caption(tag).wrapping(Wrapping::None))
        .padding([0, 6])
        .height(Length::Fixed(18.0))
        .center_y(Length::Fixed(18.0))
        .style(control::tag)
        .into()
}

/// Which glyph a status shows (`.st .gk/.gw/.gx/.gu/.go/.gb`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Ok,
    Warn,
    Bad,
    Unknown,
    Off,
    Busy,
}

/// A status's glyph and the tone its glyph and text take.
pub fn status_glyph(kind: Status) -> (Text<'static>, Tone) {
    match kind {
        Status::Ok => (icon::check_circle_icon(), Tone::Good),
        Status::Warn => (icon::exclamation_circle_icon(), Tone::Warn),
        Status::Bad => (icon::x_circle_icon(), Tone::Bad),
        Status::Unknown => (icon::question_circle_icon(), Tone::Muted),
        Status::Off => (icon::dash_circle_icon(), Tone::Muted),
        Status::Busy => (icon::dot_icon(), Tone::Muted),
    }
}

/// The status line under a name saved only on this device: a save in flight
/// or a failed one first (the field keeps the submitted text until the save
/// lands), then an unsaved edit, then the last save. `None` until the name
/// is edited or saved this session.
pub fn local_name_status(
    unsaved: bool,
    save: crate::app::cache::NameSave,
) -> Option<(Status, &'static str)> {
    use crate::app::cache::NameSave;
    Some(match save {
        NameSave::Saving => (Status::Busy, "Saving…"),
        NameSave::Failed => (Status::Bad, "Couldn't save. Press Return to try again."),
        _ if unsaved => (Status::Warn, "Not saved yet. Press Return to save."),
        NameSave::Idle => return None,
        NameSave::Saved => (Status::Ok, "Saved on this device."),
    })
}

/// A status line (`.st`): glyph and 13px medium text in the status colour.
pub fn status<'a, M: 'a>(kind: Status, text: impl Into<Cow<'a, str>>) -> Element<'a, M> {
    let (glyph, tone) = status_glyph(kind);
    let style = move |t: &theme::Theme| theme::tenshu::text::tone(t, tone);
    Row::new()
        .spacing(6)
        .align_y(Alignment::Center)
        .push(glyph.size(16).style(style))
        .push(
            iced::widget::text(text.into())
                .size(13)
                .font(coincube_ui::font::MEDIUM)
                .wrapping(Wrapping::None)
                .style(style),
        )
        .into()
}

/// A 32px row button (`.btn`).
pub fn button<'a>(
    id: impl Into<iced::advanced::widget::Id>,
    label: impl Into<Cow<'a, str>>,
    on_press: Option<Message>,
) -> Element<'a, Message> {
    ActionButton::new(id, label, ActionKind::Secondary, Message::FocusAction)
        .compact()
        .on_press_maybe(on_press)
        .into()
}

/// The 32px accent button (`.btn.pri`).
pub fn primary<'a>(
    id: impl Into<iced::advanced::widget::Id>,
    label: impl Into<Cow<'a, str>>,
    on_press: Option<Message>,
) -> Element<'a, Message> {
    ActionButton::new(id, label, ActionKind::Primary, Message::FocusAction)
        .compact()
        .on_press_maybe(on_press)
        .into()
}

/// The lead block (`.lead`): a 2px warning rule above, a status, a 20px title
/// and a description beside the main action, ruled below.
pub fn lead<'a, M: 'a>(
    kind: Status,
    status_text: impl Into<Cow<'a, str>>,
    title: impl Into<Cow<'a, str>>,
    detail: impl Into<Cow<'a, str>>,
    action: Element<'a, M>,
) -> Element<'a, M> {
    let warn = kind == Status::Warn;
    let body = Row::new()
        .spacing(24)
        .align_y(Alignment::Center)
        .push(
            Column::new()
                .width(Length::Fill)
                .push(status(kind, status_text))
                .push(Space::new().height(Length::Fixed(6.0)))
                .push(tenshu::heading(title.into()))
                .push(Space::new().height(Length::Fixed(2.0)))
                .push(muted(detail)),
        )
        .push(action);
    Column::new()
        .width(Length::Fill)
        .push(Space::new().height(Length::Fixed(24.0)))
        .push(
            Container::new(Space::new())
                .width(Length::Fill)
                .height(Length::Fixed(2.0))
                .style(move |t: &theme::Theme| iced::widget::container::Style {
                    background: Some(iced::Background::Color(if warn {
                        t.colors.shell.warn
                    } else {
                        t.colors.shell.line
                    })),
                    ..Default::default()
                }),
        )
        .push(Container::new(body).padding([18, 0]))
        .push(rule())
        .into()
}

/// A bordered note (`.note`): warning outline by default, `.bad` or `.info`.
pub fn note<'a, M: 'a>(
    tone: Tone,
    content: impl Into<Element<'a, M>>,
    actions: Option<Element<'a, M>>,
) -> Element<'a, M> {
    let body = Row::new()
        .spacing(16)
        .align_y(Alignment::Center)
        .push(Container::new(content).width(Length::Fill));
    let body = match actions {
        Some(actions) => body.push(actions),
        None => body,
    };
    Column::new()
        .push(Space::new().height(Length::Fixed(12.0)))
        .push(
            Container::new(body)
                .padding([12, 14])
                .width(Length::Fill)
                .style(move |t: &theme::Theme| {
                    let shell = &t.colors.shell;
                    iced::widget::container::Style {
                        border: iced::Border {
                            color: match tone {
                                Tone::Bad => shell.bad,
                                Tone::Warn => shell.warn,
                                _ => shell.line,
                            },
                            width: 1.0,
                            radius: 8.0.into(),
                        },
                        ..Default::default()
                    }
                }),
        )
        .into()
}

/// The right-aligned status under a form (`.saved`); keeps its 26px even when
/// empty so rows below do not move.
pub fn saved_line<'a, M: 'a>(saved: bool) -> Element<'a, M> {
    Container::new(muted(if saved { localized_copy("Saved") } else { "" }))
        .width(Length::Fill)
        .height(Length::Fixed(26.0))
        .padding(iced::Padding::default().top(8.0))
        .align_x(iced::alignment::Horizontal::Right)
        .into()
}

/// A muted 12px caption paragraph below a group (`.cap`).
pub fn caption<'a, M: 'a>(content: impl Into<Cow<'a, str>>) -> Element<'a, M> {
    Column::new()
        .push(Space::new().height(Length::Fixed(12.0)))
        .push(tenshu::caption(content.into()))
        .into()
}

/// A disclosure header (`.dsc`): chevron, 15px semibold title and a muted
/// caption, ruled above and below, 32px below the content before it.
pub fn disclosure<'a>(
    id: &'static str,
    title: &'a str,
    caption: Option<&'a str>,
    open: bool,
    on_toggle: Message,
) -> Element<'a, Message> {
    let mut label = Row::new()
        .spacing(8)
        .align_y(Alignment::Center)
        .push(
            if open {
                icon::down_icon()
            } else {
                icon::collapsed_icon()
            }
            .size(14)
            .style(|t: &theme::Theme| theme::tenshu::text::tone(t, Tone::Muted)),
        )
        .push(
            iced::widget::text(title)
                .size(15)
                .font(coincube_ui::font::SEMIBOLD),
        );
    if let Some(caption) = caption {
        label = label.push(muted(caption));
    }
    Column::new()
        .push(Space::new().height(Length::Fixed(GROUP_GAP)))
        .push(rule())
        .push(
            coincube_ui::widget::action::Action::new(
                id,
                Container::new(label).padding([12, 0]).width(Length::Fill),
                Message::FocusAction,
            )
            .style(theme::tenshu::control::disclosure)
            .width(Length::Fill)
            .padding(0)
            .on_press(on_toggle),
        )
        .push(rule())
        .into()
}

/// The disclosure body (`.dbody`): inset 26px; its rows drop the top rule.
pub fn disclosure_body<'a, M: 'a>(
    items: impl IntoIterator<Item = Element<'a, M>>,
) -> Element<'a, M> {
    Container::new(
        items
            .into_iter()
            .fold(Column::new().width(Length::Fill), |c, item| c.push(item)),
    )
    .padding(iced::Padding::default().left(26.0))
    .into()
}

/// An accent text button (`.lnk`).
pub fn link<'a>(id: &'static str, label: &'a str, on_press: Message) -> Element<'a, Message> {
    ActionButton::new(id, label, ActionKind::Quiet, Message::FocusAction)
        .compact()
        .on_press(on_press)
        .into()
}

/// A 32px row button for panels with their own message type (`.btn`,
/// `.btn.pri`). Prefer [`button`] where the panel speaks the view message,
/// which also joins keyboard focus order.
pub fn plain_button<'a, M: Clone + 'a>(
    label: impl Into<Cow<'a, str>>,
    kind: ActionKind,
    on_press: Option<M>,
) -> Element<'a, M> {
    use coincube_ui::theme::tenshu::button::{action, ActionShape};
    iced::widget::button(
        Container::new(
            iced::widget::text(label.into())
                .size(13)
                .font(coincube_ui::font::MEDIUM)
                .wrapping(Wrapping::None),
        )
        .height(Length::Fill)
        .center_y(Length::Fill),
    )
    .height(Length::Fixed(32.0))
    .padding([0, 12])
    .style(move |t, status| action(t, status, kind, ActionShape::Compact))
    .on_press_maybe(on_press)
    .into()
}

/// A segmented choice (`.seg`) for panels with their own message type: 28px
/// segments in a 1px frame; the selected one takes the selected fill.
pub fn plain_segments<'a, M: Clone + 'a>(
    items: impl IntoIterator<Item = (&'a str, bool, M)>,
) -> Element<'a, M> {
    let row = items.into_iter().fold(
        Row::new().spacing(2).align_y(Alignment::Center),
        |row, (label, selected, message)| {
            row.push(
                iced::widget::button(
                    Container::new(
                        iced::widget::text(label)
                            .size(13)
                            .font(if selected {
                                coincube_ui::font::SEMIBOLD
                            } else {
                                coincube_ui::font::MEDIUM
                            })
                            .wrapping(Wrapping::None),
                    )
                    .height(Length::Fill)
                    .center_y(Length::Fill),
                )
                .height(Length::Fixed(28.0))
                .padding([0, 12])
                .style(move |t, status| {
                    let mut style = control::segment(t, status, selected, 6.0);
                    style.border.width = 0.0;
                    style
                })
                .on_press(message),
            )
        },
    );
    Container::new(row)
        .padding(2)
        .style(|t| control::segmented_frame(t, 8.0))
        .into()
}

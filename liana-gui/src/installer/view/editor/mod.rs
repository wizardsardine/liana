#![allow(deprecated)]

pub mod template;

use iced::{
    alignment,
    widget::{column, row, slider, Space},
    Alignment, Length,
};

use liana_ui::component::{
    button::{btn_chevron, btn_edit, btn_remove, btn_set},
    text::{p1_bold, p2_regular, H3_SIZE},
};
use std::borrow::Cow;
use std::fmt::Display;
use std::str::FromStr;

use liana_ui::{
    component::{
        button, card,
        checkbox::labelled_radio,
        form,
        text::{new, p1_regular, text, Text},
        tooltip,
    },
    icon,
    spacing::{HSpacing, VSpacing},
    theme,
    widget::*,
};

use crate::installer::{
    descriptor::{PathKind, PathSequence, PathWarning},
    message::{self, Message},
    view::defined_sequence,
};
use crate::t;

use super::defined_threshold;

fn descriptor_type_label(use_taproot: bool) -> String {
    if use_taproot {
        t!("installer-descriptor-type-taproot")
    } else {
        t!("installer-descriptor-type-segwit")
    }
}

/// The descriptor type as a status line: the current choice with a tooltip, and
/// a chevron unfolding the two options.
pub fn descriptor_type<'a>(use_taproot: bool, editing: bool) -> Element<'a, Message> {
    let chevron = btn_chevron(editing, Message::ShowDescriptorTypeOptions(!editing));
    let status = row![
        new::caption(t!("installer-descriptor-type")).style(theme::text::secondary),
        tooltip::tooltip_with_style(
            t!("installer-descriptor-type-tooltip"),
            theme::text::secondary,
        ),
        new::b5_bold(descriptor_type_label(use_taproot)),
        Space::with_width(HSpacing::XS),
        chevron
    ]
    .spacing(HSpacing::M)
    .align_y(Alignment::Center);

    let taproot = labelled_radio(
        descriptor_type_label(true),
        use_taproot,
        Message::CreateTaprootDescriptor(true),
    );
    let segwit = labelled_radio(
        descriptor_type_label(false),
        !use_taproot,
        Message::CreateTaprootDescriptor(false),
    );
    let options = editing.then(|| column![taproot, segwit].spacing(HSpacing::M));

    column![status]
        .push_maybe(options)
        .spacing(VSpacing::SM)
        .into()
}

pub fn path(
    color: iced::Color,
    title: Option<String>,
    sequence: PathSequence,
    warning: Option<PathWarning>,
    threshold: usize,
    keys: Vec<Element<message::DefinePath>>,
    fixed: bool,
) -> Element<message::DefinePath> {
    let keys_len = keys.len();
    Container::new(
        Column::new()
            .spacing(10)
            .push_maybe(title.map(|t| Row::new().push(Space::with_width(10)).push(p1_bold(t))))
            .push(defined_sequence(sequence, warning))
            .push(
                Column::new()
                    .spacing(5)
                    .align_x(Alignment::Center)
                    .push(Column::with_children(keys).spacing(5)),
            )
            .push_maybe(if fixed {
                if keys_len == 1 {
                    None
                } else {
                    Some(Row::new().push(defined_threshold(color, fixed, (threshold, keys_len))))
                }
            } else {
                Some(
                    Row::new()
                        .spacing(10)
                        .push(defined_threshold(color, fixed, (threshold, keys_len)))
                        .push(
                            button::secondary(
                                Some(icon::plus_icon()),
                                if sequence.path_kind() == PathKind::SafetyNet {
                                    t!("installer-add-safety-net-key")
                                } else {
                                    t!("installer-add-key")
                                },
                            )
                            .on_press(message::DefinePath::AddKey),
                        ),
                )
            }),
    )
    .padding(10)
    .style(theme::card::border)
    .into()
}

/// An info icon next to a key name, revealing on hover a note linking to the
/// help page behind it.
fn key_note<'a>(text: String, url: String) -> Element<'a, message::DefineKey> {
    tooltip::tooltip_interactive(
        button::subtle_link(text, Some(message::DefineKey::OpenUrl(url))),
        icon::tooltip_icon().style(theme::text::secondary),
    )
}

pub fn uneditable_defined_key<'a>(
    alias: &'a str,
    color: iced::Color,
    title: impl Into<Cow<'a, str>> + std::fmt::Display,
    warning: Option<String>,
    note: Option<(String /* text */, String /* url */)>,
) -> Element<'a, message::DefineKey> {
    let valid = warning.is_none();
    card::simple(
        Row::new()
            .spacing(10)
            .width(Length::Fill)
            .align_y(Alignment::Center)
            .push(icon::round_key_icon().size(H3_SIZE).color(color))
            .push(
                Column::new()
                    .width(Length::Fill)
                    .spacing(5)
                    .push(
                        Row::new()
                            .spacing(10)
                            .push(p1_regular(title).style(theme::text::secondary))
                            .push(p1_bold(alias))
                            .push_maybe(note.map(|(text, url)| key_note(text, url))),
                    )
                    .push_maybe(warning.map(|w| p2_regular(w).style(theme::text::error))),
            )
            .push_maybe(if valid {
                Some(icon::check_icon().style(theme::text::success))
            } else {
                None
            }),
    )
    .into()
}

pub fn defined_key<'a>(
    alias: &'a str,
    color: iced::Color,
    title: impl Display,
    warning: Option<String>,
    note: Option<(String /* text */, String /* url */)>,
    fixed: bool,
) -> Element<'a, message::DefineKey> {
    let valid = warning.is_none();
    let delete_button = (!fixed).then_some(btn_remove(Some(message::DefineKey::Delete)));
    let edit_button = btn_edit(Some(message::DefineKey::EditAlias));
    card::simple(
        Row::new()
            .spacing(10)
            .width(Length::Fill)
            .align_y(Alignment::Center)
            .push(icon::round_key_icon().size(H3_SIZE).color(color))
            .push(
                Column::new()
                    .width(Length::Fill)
                    .spacing(5)
                    .push(
                        Row::new()
                            .spacing(10)
                            .push(p1_regular(format!("{title}")).style(theme::text::secondary))
                            .push(p1_bold(alias))
                            .push_maybe(note.map(|(text, url)| key_note(text, url))),
                    )
                    .push_maybe(warning.map(|w| p2_regular(w).style(theme::text::error))),
            )
            .push_maybe(if valid {
                Some(icon::check_icon().style(theme::text::success))
            } else {
                None
            })
            .push(edit_button)
            .push_maybe(delete_button),
    )
    .into()
}

pub fn undefined_key<'a>(
    color: iced::Color,
    title: impl Into<Cow<'a, str>> + std::fmt::Display,
    active: bool,
    fixed: bool,
) -> Element<'a, message::DefineKey> {
    let delete_button = (!fixed).then_some(btn_remove(Some(message::DefineKey::Delete)));
    let set_button = active.then_some(btn_set(Some(message::DefineKey::Edit)));
    card::simple(
        Row::new()
            .spacing(10)
            .width(Length::Fill)
            .align_y(Alignment::Center)
            .push(icon::round_key_icon().size(H3_SIZE).color(color))
            .push(
                Column::new()
                    .width(Length::Fill)
                    .spacing(5)
                    .push(p1_bold(title)),
            )
            .push_maybe(set_button)
            .push_maybe(delete_button),
    )
    .into()
}

/// returns y,m,d,h,m
fn duration_from_sequence(sequence: u16) -> (u32, u32, u32, u32, u32) {
    let mut n_minutes = sequence as u32 * 10;
    let n_years = n_minutes / 525960;
    n_minutes -= n_years * 525960;
    let n_months = n_minutes / 43830;
    n_minutes -= n_months * 43830;
    let n_days = n_minutes / 1440;
    n_minutes -= n_days * 1440;
    let n_hours = n_minutes / 60;
    n_minutes -= n_hours * 60;

    (n_years, n_months, n_days, n_hours, n_minutes)
}

/// Formats a Bitcoin sequence duration into readable units with smart truncation.
///
/// Converts block count to (value, unit) tuples and truncates precision based on duration:
/// - ≥ 1440 blocks (~10d): show up to days (e.g., "1m 10d")
/// - 144-1439 blocks (~1-10d): show up to hours (e.g., "2d 5h")
/// - < 144 blocks: show all units (e.g., "3h 45mn")
///
/// `short_format`: true = "y/m/d/h/mn", false = "year/month/day/hour/minute"
pub fn format_sequence_duration(sequence: u16, short_format: bool) -> Vec<(u32, String)> {
    let (n_years, n_months, n_days, n_hours, n_minutes) = duration_from_sequence(sequence);

    let mut formatted_duration = if short_format {
        vec![
            (n_years, t!("duration-years-compact", count = n_years)),
            (n_months, t!("duration-months-compact", count = n_months)),
            (n_days, t!("duration-days-compact", count = n_days)),
            (n_hours, t!("duration-hours-compact", count = n_hours)),
            (n_minutes, t!("duration-minutes-compact", count = n_minutes)),
        ]
    } else {
        vec![
            (n_years, t!("duration-years", count = n_years)),
            (n_months, t!("duration-months", count = n_months)),
            (n_days, t!("duration-days", count = n_days)),
            (n_hours, t!("duration-hours", count = n_hours)),
            (n_minutes, t!("duration-minutes", count = n_minutes)),
        ]
    };

    if sequence >= 1440 {
        formatted_duration.truncate(3);
    } else if sequence >= 144 {
        formatted_duration.truncate(4);
    }

    formatted_duration
}

pub fn edit_sequence_modal<'a>(sequence: &form::Value<String>) -> Element<'a, Message> {
    let mut col = Column::new()
        .width(Length::Fill)
        .spacing(20)
        .align_x(Alignment::Center)
        .push(text(t!("installer-keys-inactivity")))
        .push(
            Row::new()
                .push(
                    Container::new(
                        form::Form::new_trimmed("ex: 1000", sequence, |v| {
                            Message::DefineDescriptor(
                                message::DefineDescriptor::ThresholdSequenceModal(
                                    message::ThresholdSequenceModal::SequenceEdited(v),
                                ),
                            )
                        })
                        .warning(t!("installer-sequence-value-warning")),
                    )
                    .width(Length::Fixed(200.0)),
                )
                .spacing(10)
                .push(text(t!("common-blocks")).bold())
                .align_y(alignment::Vertical::Center),
        );

    if sequence.valid {
        if let Ok(sequence) = u16::from_str(&sequence.value) {
            col = col
                .push(format_sequence_duration(sequence, false).iter().fold(
                    Row::new().spacing(5).push(text("~ ").bold()),
                    |row, (n, unit)| {
                        row.push_maybe(if *n > 0 {
                            Some(text(unit).bold())
                        } else {
                            None
                        })
                    },
                ))
                .push(
                    Container::new(
                        slider(1..=u16::MAX, sequence, |v| {
                            Message::DefineDescriptor(
                                message::DefineDescriptor::ThresholdSequenceModal(
                                    message::ThresholdSequenceModal::SequenceEdited(
                                        // Since slider starts at 1, intermediate values are off by 1 from intended values.
                                        // Subtract 1 to align with expected sequence values, except for edge cases (1 and u16::MAX)
                                        (if v > 1 && v != u16::MAX { v - 1 } else { v })
                                            .to_string(),
                                    ),
                                ),
                            )
                        })
                        .step(4383_u16), // 4383 blocks per month
                    )
                    .width(Length::Fixed(500.0)),
                );
        }
    }

    card::modal(col.push(if sequence.valid {
        button::primary(None, t!("btn-apply"))
            .on_press(Message::DefineDescriptor(
                message::DefineDescriptor::ThresholdSequenceModal(
                    message::ThresholdSequenceModal::Confirm,
                ),
            ))
            .width(Length::Fixed(200.0))
    } else {
        button::primary(None, t!("btn-apply")).width(Length::Fixed(200.0))
    }))
    .width(Length::Fixed(800.0))
    .into()
}

pub fn edit_threshold_modal<'a>(threshold: (usize, usize)) -> Element<'a, Message> {
    card::modal(
        Column::new()
            .width(Length::Fill)
            .spacing(20)
            .align_x(Alignment::Center)
            .push(threshsold_input::threshsold_input(
                threshold.0,
                threshold.1,
                |v| {
                    Message::DefineDescriptor(message::DefineDescriptor::ThresholdSequenceModal(
                        message::ThresholdSequenceModal::ThresholdEdited(v),
                    ))
                },
            ))
            .push(
                button::primary(None, t!("btn-apply"))
                    .on_press(Message::DefineDescriptor(
                        message::DefineDescriptor::ThresholdSequenceModal(
                            message::ThresholdSequenceModal::Confirm,
                        ),
                    ))
                    .width(Length::Fixed(200.0)),
            ),
    )
    .width(Length::Fixed(800.0))
    .into()
}

mod threshsold_input {
    use iced::alignment::{self, Alignment};
    use iced::widget::{component, Component};
    use iced::Length;
    use liana_ui::{component::text::*, icon, theme, widget::*};

    pub struct ThresholdInput<Message> {
        value: usize,
        max: usize,
        on_change: Box<dyn Fn(usize) -> Message>,
    }

    pub fn threshsold_input<Message>(
        value: usize,
        max: usize,
        on_change: impl Fn(usize) -> Message + 'static,
    ) -> ThresholdInput<Message> {
        ThresholdInput::new(value, max, on_change)
    }

    #[derive(Debug, Clone)]
    pub enum Event {
        IncrementPressed,
        DecrementPressed,
    }

    impl<Message> ThresholdInput<Message> {
        pub fn new(
            value: usize,
            max: usize,
            on_change: impl Fn(usize) -> Message + 'static,
        ) -> Self {
            Self {
                value,
                max,
                on_change: Box::new(on_change),
            }
        }
    }

    impl<Message> Component<Message, theme::Theme> for ThresholdInput<Message> {
        type State = ();
        type Event = Event;

        fn update(&mut self, _state: &mut Self::State, event: Event) -> Option<Message> {
            match event {
                Event::IncrementPressed => {
                    if self.value < self.max {
                        Some((self.on_change)(self.value.saturating_add(1)))
                    } else {
                        None
                    }
                }
                Event::DecrementPressed => {
                    if self.value > 1 {
                        Some((self.on_change)(self.value.saturating_sub(1)))
                    } else {
                        None
                    }
                }
            }
        }

        fn view(&self, _state: &Self::State) -> Element<'_, Self::Event> {
            let button = |label, on_press| {
                Button::new(label)
                    .style(theme::button::transparent)
                    .width(Length::Fixed(50.0))
                    .on_press(on_press)
            };

            Column::new()
                .width(Length::Fixed(150.0))
                .push(button(icon::up_icon().size(30), Event::IncrementPressed))
                .push(text(crate::t!("installer-threshold")).small().bold())
                .push(
                    Container::new(text(format!("{}/{}", self.value, self.max)).size(30))
                        .align_y(alignment::Vertical::Center),
                )
                .push(button(icon::down_icon().size(30), Event::DecrementPressed))
                .align_x(Alignment::Center)
                .into()
        }
    }

    impl<'a, Message> From<ThresholdInput<Message>> for Element<'a, Message>
    where
        Message: 'a,
    {
        fn from(numeric_input: ThresholdInput<Message>) -> Self {
            component(numeric_input)
        }
    }
}

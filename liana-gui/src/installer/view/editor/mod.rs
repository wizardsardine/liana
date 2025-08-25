#![allow(deprecated)]

pub mod template;

use iced::widget::{container, pick_list, slider, Button, Space};
use iced::{alignment, Alignment, Length};

use liana::miniscript::bitcoin::Network;
use liana_ui::component::text::{p1_bold, p2_regular, H3_SIZE};
use std::borrow::Cow;
use std::fmt::Display;
use std::str::FromStr;

use liana::miniscript::bitcoin::{self};
use liana_ui::{
    component::{
        button, card, form, separation,
        text::{p1_regular, text, Text},
    },
    icon, theme,
    widget::*,
};

use crate::installer::{
    descriptor::{PathKind, PathSequence, PathWarning},
    message::{self, Message},
    view::defined_sequence,
};

use super::defined_threshold;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DescriptorKind {
    P2WSH,
    Taproot,
}

const DESCRIPTOR_KINDS: [DescriptorKind; 2] = [DescriptorKind::P2WSH, DescriptorKind::Taproot];

impl std::fmt::Display for DescriptorKind {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::P2WSH => write!(f, "P2WSH"),
            Self::Taproot => write!(f, "Taproot"),
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn define_descriptor_advanced_settings<'a>(use_taproot: bool) -> Element<'a, Message> {
    let col_wallet = Column::new()
        .spacing(10)
        .push(text("Descriptor type").bold())
        .push(container(
            pick_list(
                &DESCRIPTOR_KINDS[..],
                Some(if use_taproot {
                    DescriptorKind::Taproot
                } else {
                    DescriptorKind::P2WSH
                }),
                |kind| Message::CreateTaprootDescriptor(kind == DescriptorKind::Taproot),
            )
            .style(theme::pick_list::primary)
            .padding(10),
        ));

    container(
        Column::new()
            .spacing(20)
            .push(Space::with_height(0))
            .push(separation().width(500))
            .push(Row::new().push(col_wallet))
            .push_maybe(if use_taproot {
                Some(
                    p1_regular("Taproot is only supported by Liana version 5.0 and above")
                        .style(theme::text::secondary),
                )
            } else {
                None
            }),
    )
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
                                    "Add Safety Net key"
                                } else {
                                    "Add key"
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

pub fn uneditable_defined_key<'a>(
    alias: &'a str,
    color: iced::Color,
    title: impl Into<Cow<'a, str>> + std::fmt::Display,
    warning: Option<&'static str>,
) -> Element<'a, message::DefineKey> {
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
                            .push(p1_bold(alias)),
                    )
                    .push_maybe(warning.map(|w| p2_regular(w).style(theme::text::error))),
            )
            .push_maybe(if warning.is_none() {
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
    warning: Option<&'static str>,
    fixed: bool,
) -> Element<'a, message::DefineKey> {
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
                            .push(p1_regular(format!("{}", title)).style(theme::text::secondary))
                            .push(p1_bold(alias)),
                    )
                    .push_maybe(warning.map(|w| p2_regular(w).style(theme::text::error))),
            )
            .push_maybe(if warning.is_none() {
                Some(icon::check_icon().style(theme::text::success))
            } else {
                None
            })
            .push(
                button::secondary(Some(icon::pencil_icon()), "Edit")
                    .on_press(message::DefineKey::EditAlias),
            )
            .push_maybe(if fixed {
                None
            } else {
                Some(
                    Button::new(icon::trash_icon())
                        .style(theme::button::secondary)
                        .padding(5)
                        .on_press(message::DefineKey::Delete),
                )
            }),
    )
    .into()
}

pub fn undefined_key<'a>(
    color: iced::Color,
    title: impl Into<Cow<'a, str>> + std::fmt::Display,
    active: bool,
    fixed: bool,
) -> Element<'a, message::DefineKey> {
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
            .push_maybe(if active {
                Some(
                    button::primary(Some(icon::pencil_icon()), "Set")
                        .on_press(message::DefineKey::Edit),
                )
            } else {
                None
            })
            .push_maybe(if fixed {
                None
            } else {
                Some(
                    Button::new(icon::trash_icon())
                        .style(theme::button::secondary)
                        .padding(5)
                        .on_press(message::DefineKey::Delete),
                )
            }),
    )
    .into()
}

pub fn example_xpub(network: Network) -> String {
    format!("[aabbccdd/42'/0']{}pub6DAkq8LWw91WGgUGnkR5Sbzjev5JCsXaTVZQ9MwsPV4BkNFKygtJ8GHodfDVx1udR723nT7JASqGPpKvz7zQ25pUTW6zVEBdiWoaC4aUqik",
        if network == bitcoin::Network::Bitcoin { "x" } else { "t" }
    )
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
pub fn format_sequence_duration(sequence: u16, short_format: bool) -> Vec<(u32, &'static str)> {
    let (n_years, n_months, n_days, n_hours, n_minutes) = duration_from_sequence(sequence);

    let mut formatted_duration = if short_format {
        vec![
            (n_years, "y"),
            (n_months, "m"),
            (n_days, "d"),
            (n_hours, "h"),
            (n_minutes, "mn"),
        ]
    } else {
        vec![
            (n_years, "year"),
            (n_months, "month"),
            (n_days, "day"),
            (n_hours, "hour"),
            (n_minutes, "minute"),
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
        .push(text("Keys can move the funds after inactivity of:"))
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
                        .warning("Value must be superior to 0 and inferior to 65535"),
                    )
                    .width(Length::Fixed(200.0)),
                )
                .spacing(10)
                .push(text("blocks").bold())
                .align_y(alignment::Vertical::Center),
        );

    if sequence.valid {
        if let Ok(sequence) = u16::from_str(&sequence.value) {
            col = col
                .push(format_sequence_duration(sequence, false).iter().fold(
                    Row::new().spacing(5).push(text("~ ").bold()),
                    |row, (n, unit)| {
                        row.push_maybe(if *n > 0 {
                            Some(
                                text(format!("{} {}{}", n, unit, if *n > 1 { "s" } else { "" }))
                                    .bold(),
                            )
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
        button::primary(None, "Apply")
            .on_press(Message::DefineDescriptor(
                message::DefineDescriptor::ThresholdSequenceModal(
                    message::ThresholdSequenceModal::Confirm,
                ),
            ))
            .width(Length::Fixed(200.0))
    } else {
        button::primary(None, "Apply").width(Length::Fixed(200.0))
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
                button::primary(None, "Apply")
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

        fn view(&self, _state: &Self::State) -> Element<Self::Event> {
            let button = |label, on_press| {
                Button::new(label)
                    .style(theme::button::transparent)
                    .width(Length::Fixed(50.0))
                    .on_press(on_press)
            };

            Column::new()
                .width(Length::Fixed(150.0))
                .push(button(icon::up_icon().size(30), Event::IncrementPressed))
                .push(text("Threshold:").small().bold())
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

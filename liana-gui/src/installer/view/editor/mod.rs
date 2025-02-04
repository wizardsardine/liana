#![allow(deprecated)]

pub mod template;

use iced::widget::{container, pick_list, scrollable, slider, Button, Space};
use iced::{Alignment, Length};

use liana::miniscript::bitcoin::Network;
use liana_ui::component::text::{self, h3, p1_bold, p2_regular, H3_SIZE};
use liana_ui::image;
use std::borrow::Cow;
use std::fmt::Display;
use std::str::FromStr;

use liana::miniscript::bitcoin::{self, bip32::Fingerprint};
use liana_ui::{
    component::{
        button, card, form, hw, separation,
        text::{p1_regular, text, Text},
        tooltip,
    },
    icon, theme,
    widget::*,
};

use crate::installer::{
    message::{self, Message},
    prompt,
    view::defined_sequence,
    Error,
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
    sequence: u16,
    duplicate_sequence: bool,
    threshold: usize,
    keys: Vec<Element<message::DefinePath>>,
    fixed: bool,
) -> Element<message::DefinePath> {
    let keys_len = keys.len();
    Container::new(
        Column::new()
            .spacing(10)
            .push_maybe(title.map(|t| Row::new().push(Space::with_width(10)).push(p1_bold(t))))
            .push(defined_sequence(sequence, duplicate_sequence))
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
                            button::secondary(Some(icon::plus_icon()), "Add key")
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
                    .on_press(message::DefineKey::Edit),
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

#[allow(clippy::too_many_arguments)]
pub fn edit_key_modal<'a>(
    title: &'a str,
    network: bitcoin::Network,
    hws: Vec<Element<'a, Message>>,
    keys: Vec<Element<'a, Message>>,
    error: Option<&Error>,
    chosen_signer: Option<Fingerprint>,
    hot_signer_fingerprint: &Fingerprint,
    signer_alias: Option<&'a String>,
    form_name: &'a form::Value<String>,
    form_xpub: &form::Value<String>,
    manually_imported_xpub: bool,
    duplicate_master_fg: bool,
) -> Element<'a, Message> {
    let content = Column::new()
        .padding(25)
        .push_maybe(error.map(|e| card::error("Failed to import xpub", e.to_string())))
        .push(card::modal(
            Column::new()
                .spacing(25)
                .push(Row::new()
                    .push(h3(title))
                    .push(Space::with_width(Length::Fill))
                    .push(button::transparent(Some(icon::cross_icon().size(40)), "").on_press(Message::Close))
                    .align_y(Alignment::Center)
                )
                .push(
                    Column::new()
                        .push(p1_regular("Select the signing device for your key"))
                        .spacing(10)
                        .push(
                            Column::with_children(hws).spacing(10)
                        )
                        .push(
                            Column::with_children(keys).spacing(10)
                        )
                        .push(
                            Button::new(if Some(*hot_signer_fingerprint) == chosen_signer {
                                hw::selected_hot_signer(hot_signer_fingerprint, signer_alias)
                            } else {
                                hw::unselected_hot_signer(hot_signer_fingerprint, signer_alias)
                            })
                            .width(Length::Fill)
                            .on_press(Message::UseHotSigner)
                            .style(theme::button::container_border),
                        )
                        .push(if manually_imported_xpub {
                                card::simple(Column::new()
                                    .spacing(10)
                                    .push(
                                        Row::new()
                                            .align_y(Alignment::Center)
                                            .push(p1_regular("Enter an extended public key:").width(Length::Fill))
                                            .push(image::success_mark_icon().width(Length::Fixed(50.0)))
                                    )
                                    .push(
                                        Row::new()
                                            .push(
                                                form::Form::new_trimmed(
                                                    &example_xpub(network),
                                                    form_xpub, |msg| {
                                                        Message::DefineDescriptor(
                                                            message::DefineDescriptor::KeyModal(
                                                                message::ImportKeyModal::XPubEdited(msg),),)
                                                    })
                                                    .warning(if network == bitcoin::Network::Bitcoin {
                                                        "Please enter correct xpub with origin and without appended derivation path"
                                                    } else {
                                                        "Please enter correct tpub with origin and without appended derivation path"
                                                    })
                                                    .size(text::P1_SIZE)
                                                    .padding(10),
                                            )
                                            .spacing(10)
                                    ))
                                    } else {
                                    Container::new(
                                            Button::new(
                                            Row::new()
                                                .align_y(Alignment::Center)
                                                .spacing(10)
                                                .push(icon::import_icon())
                                                .push(p1_regular("Enter an extended public key"))
                                            )
                                            .padding(20)
                                            .width(Length::Fill)
                                            .on_press(Message::DefineDescriptor(
                                                    message::DefineDescriptor::KeyModal(message::ImportKeyModal::ManuallyImportXpub)
                                            ))
                                            .style(theme::button::secondary),
                                    )
                                }
                        )
                        .width(Length::Fill),
                )
                .push_maybe(
                    if chosen_signer.is_some() {
                        Some(card::simple(Column::new()
                            .spacing(10)
                            .push(
                                Row::new()
                                    .spacing(5)
                                    .push(text("Key name:").bold())
                                    .push(tooltip(prompt::DEFINE_DESCRIPTOR_FINGERPRINT_TOOLTIP)),
                            )
                            .push(p1_regular("Give this key a friendly name. It helps you identify it later").style(theme::text::secondary))
                            .push(
                                form::Form::new("Name", form_name, |msg| {
                                    Message::DefineDescriptor(message::DefineDescriptor::KeyModal(
                                        message::ImportKeyModal::NameEdited(msg),
                                    ))
                                })
                                .warning("Two different keys cannot have the same name")
                                .padding(10)
                                .size(text::P1_SIZE)
                            )))
                    } else {
                        None
                    }
                )
                .push_maybe(
                    if duplicate_master_fg {
                        Some(text("A single signing device may not be used more than once per path. (It can still be used in other paths.)").style(theme::text::error))
                    } else {
                        None
                    }
                )
                .push(
                    button::primary(None, "Apply")
                        .on_press_maybe(if !duplicate_master_fg
                            && (!manually_imported_xpub || form_xpub.valid)
                            && !form_name.value.is_empty() && form_name.valid {
                            Some(Message::DefineDescriptor(
                                message::DefineDescriptor::KeyModal(
                                    message::ImportKeyModal::ConfirmXpub,
                                ),
                            ))
                        } else {None})
                        .width(Length::Fixed(200.0))
                )
                .align_x(Alignment::Center),
        ))
        .width(Length::Fixed(800.0));
    scrollable(content).into()
}

fn example_xpub(network: Network) -> String {
    format!("[aabbccdd/42'/0']{}pub6DAkq8LWw91WGgUGnkR5Sbzjev5JCsXaTVZQ9MwsPV4BkNFKygtJ8GHodfDVx1udR723nT7JASqGPpKvz7zQ25pUTW6zVEBdiWoaC4aUqik",
        if network == bitcoin::Network::Bitcoin { "x" } else { "t" }
    )
}

/// returns y,m,d,h,m
pub fn duration_from_sequence(sequence: u16) -> (u32, u32, u32, u32, u32) {
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
                        .warning("Sequence must be superior to 0 and inferior to 65535"),
                    )
                    .width(Length::Fixed(200.0)),
                )
                .spacing(10)
                .push(text("blocks").bold()),
        );

    if sequence.valid {
        if let Ok(sequence) = u16::from_str(&sequence.value) {
            let (n_years, n_months, n_days, n_hours, n_minutes) = duration_from_sequence(sequence);
            col = col
                .push(
                    [
                        (n_years, "year"),
                        (n_months, "month"),
                        (n_days, "day"),
                        (n_hours, "hour"),
                        (n_minutes, "minute"),
                    ]
                    .iter()
                    .fold(Row::new().spacing(5), |row, (n, unit)| {
                        row.push_maybe(if *n > 0 {
                            Some(
                                text(format!("{} {}{}", n, unit, if *n > 1 { "s" } else { "" }))
                                    .bold(),
                            )
                        } else {
                            None
                        })
                    }),
                )
                .push(
                    Container::new(
                        slider(1..=u16::MAX, sequence, |v| {
                            Message::DefineDescriptor(
                                message::DefineDescriptor::ThresholdSequenceModal(
                                    message::ThresholdSequenceModal::SequenceEdited(v.to_string()),
                                ),
                            )
                        })
                        .step(144_u16), // 144 blocks per day
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

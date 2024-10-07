pub mod template;

use iced::widget::{
    container, pick_list, scrollable, scrollable::Properties, slider, Button, Space,
};
use iced::{alignment, Alignment, Length};

use liana_ui::component::text;
use std::str::FromStr;

use liana::miniscript::bitcoin::{self, bip32::Fingerprint};
use liana_ui::{
    color,
    component::{
        button, card, collapse, form, hw, separation,
        text::{p1_regular, text, Text},
        tooltip,
    },
    icon, image, theme,
    widget::*,
};

use crate::installer::{
    message::{self, Message},
    prompt,
    view::{defined_sequence, layout},
    Error,
};

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
            .style(theme::PickList::Secondary)
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
                        .style(color::GREY_2),
                )
            } else {
                None
            }),
    )
    .into()
}

#[allow(clippy::too_many_arguments)]
pub fn define_descriptor<'a>(
    progress: (usize, usize),
    email: Option<&'a str>,
    use_taproot: bool,
    paths: Vec<Element<'a, Message>>,
    valid: bool,
    error: Option<&String>,
) -> Element<'a, Message> {
    layout(
        progress,
        email,
        "Create the wallet",
        Column::new()
            .push(collapse::Collapse::new(
                || {
                    Button::new(
                        Row::new()
                            .align_items(Alignment::Center)
                            .spacing(10)
                            .push(text("Advanced settings").small().bold())
                            .push(icon::collapse_icon()),
                    )
                    .style(theme::Button::Transparent)
                },
                || {
                    Button::new(
                        Row::new()
                            .align_items(Alignment::Center)
                            .spacing(10)
                            .push(text("Advanced settings").small().bold())
                            .push(icon::collapsed_icon()),
                    )
                    .style(theme::Button::Transparent)
                },
                move || define_descriptor_advanced_settings(use_taproot),
            ))
            .push(
                Column::new()
                    .width(Length::Fill)
                    .push(
                        Column::new()
                            .spacing(25)
                            .push(Column::with_children(paths).spacing(10))
                            .push(tooltip(prompt::DEFINE_DESCRIPTOR_RECOVERY_PATH_TOOLTIP)),
                    )
                    .spacing(25),
            )
            .push(
                Row::new()
                    .spacing(10)
                    .push(
                        button::secondary(Some(icon::plus_icon()), "Add a recovery path")
                            .on_press(Message::DefineDescriptor(
                                message::DefineDescriptor::AddRecoveryPath,
                            ))
                            .width(Length::Fixed(200.0)),
                    )
                    .push(if !valid {
                        button::primary(None, "Next").width(Length::Fixed(200.0))
                    } else {
                        button::primary(None, "Next")
                            .width(Length::Fixed(200.0))
                            .on_press(Message::Next)
                    }),
            )
            .push_maybe(error.map(|e| card::error("Failed to create descriptor", e.to_string())))
            .push(Space::with_height(Length::Fixed(20.0)))
            .spacing(50),
        false,
        Some(Message::Previous),
    )
}

pub fn primary_path_view(
    primary_threshold: usize,
    primary_keys: Vec<Element<message::DefinePath>>,
) -> Element<message::DefinePath> {
    Container::new(
        Column::new().push(
            Row::new()
                .align_items(Alignment::Center)
                .push_maybe(if primary_keys.len() > 1 {
                    Some(threshsold_input::threshsold_input(
                        primary_threshold,
                        primary_keys.len(),
                        message::DefinePath::ThresholdEdited,
                    ))
                } else {
                    None
                })
                .push(
                    scrollable(
                        Row::new()
                            .spacing(5)
                            .align_items(Alignment::Center)
                            .push(Row::with_children(primary_keys).spacing(5))
                            .push(
                                Button::new(
                                    Container::new(icon::plus_icon().size(50))
                                        .width(Length::Fixed(150.0))
                                        .height(Length::Fixed(150.0))
                                        .align_y(alignment::Vertical::Center)
                                        .align_x(alignment::Horizontal::Center),
                                )
                                .width(Length::Fixed(150.0))
                                .height(Length::Fixed(150.0))
                                .style(theme::Button::TransparentBorder)
                                .on_press(message::DefinePath::AddKey),
                            )
                            .padding(5),
                    )
                    .direction(scrollable::Direction::Horizontal(
                        Properties::new().width(3).scroller_width(3),
                    )),
                ),
        ),
    )
    .padding(5)
    .style(theme::Container::Card(theme::Card::Border))
    .into()
}

pub fn recovery_path_view(
    sequence: u16,
    duplicate_sequence: bool,
    recovery_threshold: usize,
    recovery_keys: Vec<Element<message::DefinePath>>,
) -> Element<message::DefinePath> {
    Container::new(
        Column::new()
            .push(defined_sequence(sequence, duplicate_sequence))
            .push(
                Row::new()
                    .align_items(Alignment::Center)
                    .push_maybe(if recovery_keys.len() > 1 {
                        Some(threshsold_input::threshsold_input(
                            recovery_threshold,
                            recovery_keys.len(),
                            message::DefinePath::ThresholdEdited,
                        ))
                    } else {
                        None
                    })
                    .push(
                        scrollable(
                            Row::new()
                                .spacing(5)
                                .align_items(Alignment::Center)
                                .push(Row::with_children(recovery_keys).spacing(5))
                                .push(
                                    Button::new(
                                        Container::new(icon::plus_icon().size(50))
                                            .width(Length::Fixed(150.0))
                                            .height(Length::Fixed(150.0))
                                            .align_y(alignment::Vertical::Center)
                                            .align_x(alignment::Horizontal::Center),
                                    )
                                    .width(Length::Fixed(150.0))
                                    .height(Length::Fixed(150.0))
                                    .style(theme::Button::TransparentBorder)
                                    .on_press(message::DefinePath::AddKey),
                                )
                                .padding(5),
                        )
                        .direction(scrollable::Direction::Horizontal(
                            Properties::new().width(3).scroller_width(3),
                        )),
                    ),
            ),
    )
    .padding(5)
    .style(theme::Container::Card(theme::Card::Border))
    .into()
}

pub fn undefined_descriptor_key<'a>() -> Element<'a, message::DefineKey> {
    card::simple(
        Column::new()
            .width(Length::Fill)
            .align_items(Alignment::Center)
            .push(
                Row::new()
                    .align_items(Alignment::Center)
                    .push(Space::with_width(Length::Fill))
                    .push(
                        Button::new(icon::cross_icon())
                            .style(theme::Button::Transparent)
                            .on_press(message::DefineKey::Delete),
                    ),
            )
            .push(
                Container::new(
                    Column::new()
                        .spacing(15)
                        .align_items(Alignment::Center)
                        .push(image::key_mark_icon().width(Length::Fixed(30.0))),
                )
                .height(Length::Fill)
                .align_y(alignment::Vertical::Center),
            )
            .push(
                button::secondary(Some(icon::pencil_icon()), "Set")
                    .on_press(message::DefineKey::Edit),
            )
            .push(Space::with_height(Length::Fixed(5.0))),
    )
    .padding(5)
    .height(Length::Fixed(150.0))
    .width(Length::Fixed(150.0))
    .into()
}

pub fn defined_descriptor_key<'a>(
    name: String,
    duplicate_name: bool,
    incompatible_with_tapminiscript: bool,
) -> Element<'a, message::DefineKey> {
    let col = Column::new()
        .width(Length::Fill)
        .align_items(Alignment::Center)
        .push(
            Row::new()
                .align_items(Alignment::Center)
                .push(Space::with_width(Length::Fill))
                .push(
                    Button::new(icon::cross_icon())
                        .style(theme::Button::Transparent)
                        .on_press(message::DefineKey::Delete),
                ),
        )
        .push(
            Container::new(
                Column::new()
                    .spacing(10)
                    .align_items(Alignment::Center)
                    .push(
                        scrollable(
                            Column::new()
                                .push(text(name).bold())
                                .push(Space::with_height(Length::Fixed(5.0))),
                        )
                        .direction(scrollable::Direction::Horizontal(
                            Properties::new().width(5).scroller_width(5),
                        )),
                    )
                    .push(image::success_mark_icon().width(Length::Fixed(50.0)))
                    .push(Space::with_width(Length::Fixed(1.0))),
            )
            .height(Length::Fill)
            .align_y(alignment::Vertical::Center),
        )
        .push(
            button::secondary(Some(icon::pencil_icon()), "Edit").on_press(message::DefineKey::Edit),
        )
        .push(Space::with_height(Length::Fixed(5.0)));

    if duplicate_name {
        Column::new()
            .align_items(Alignment::Center)
            .push(
                card::invalid(col)
                    .padding(5)
                    .height(Length::Fixed(150.0))
                    .width(Length::Fixed(150.0)),
            )
            .push(text("Duplicate name").small().style(color::RED))
            .into()
    } else if incompatible_with_tapminiscript {
        Column::new()
            .align_items(Alignment::Center)
            .push(
                card::invalid(col)
                    .padding(5)
                    .height(Length::Fixed(150.0))
                    .width(Length::Fixed(150.0)),
            )
            .push(
                text("Taproot is not supported\nby this key device")
                    .small()
                    .style(color::RED),
            )
            .into()
    } else {
        card::simple(col)
            .padding(5)
            .height(Length::Fixed(150.0))
            .width(Length::Fixed(150.0))
            .into()
    }
}

#[allow(clippy::too_many_arguments)]
pub fn edit_key_modal<'a>(
    network: bitcoin::Network,
    hws: Vec<Element<'a, Message>>,
    keys: Vec<Element<'a, Message>>,
    error: Option<&Error>,
    chosen_signer: Option<Fingerprint>,
    hot_signer_fingerprint: &Fingerprint,
    signer_alias: Option<&'a String>,
    form_xpub: &form::Value<String>,
    form_name: &'a form::Value<String>,
    edit_name: bool,
    duplicate_master_fg: bool,
) -> Element<'a, Message> {
    Column::new()
        .push_maybe(error.map(|e| card::error("Failed to import xpub", e.to_string())))
        .push(card::simple(
            Column::new()
                .spacing(25)
                .push(
                    Column::new()
                        .push(
                            Container::new(text("Select a signing device:").bold())
                                .width(Length::Fill),
                        )
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
                            .style(theme::Button::Border),
                        )
                        .width(Length::Fill),
                )
                .push(
                    Column::new()
                        .spacing(5)
                        .push(text("Or enter an extended public key:").bold())
                        .push(
                            Row::new()
                                .push(
                                    form::Form::new_trimmed(
                                        &format!(
                                            "[aabbccdd/42'/0']{}pub6DAkq8LWw91WGgUGnkR5Sbzjev5JCsXaTVZQ9MwsPV4BkNFKygtJ8GHodfDVx1udR723nT7JASqGPpKvz7zQ25pUTW6zVEBdiWoaC4aUqik",
                                            if network == bitcoin::Network::Bitcoin {
                                                "x"
                                             } else {
                                                 "t"
                                             }
                                        ),
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
                        ),
                )
                .push(
                    if !edit_name && !form_xpub.value.is_empty() && form_xpub.valid {
                        Column::new().push(
                            Row::new()
                                .push(
                                    Column::new()
                                        .spacing(5)
                                        .width(Length::Fill)
                                        .push(
                                            Row::new()
                                                .spacing(5)
                                                .push(text("Fingerprint alias:").bold())
                                                .push(tooltip(
                                                    prompt::DEFINE_DESCRIPTOR_FINGERPRINT_TOOLTIP,
                                                )),
                                        )
                                        .push(text(&form_name.value)),
                                )
                                .push(
                                    button::secondary(Some(icon::pencil_icon()), "Edit").on_press(
                                        Message::DefineDescriptor(
                                            message::DefineDescriptor::KeyModal(
                                                message::ImportKeyModal::EditName,
                                            ),
                                        ),
                                    ),
                                ),
                        )
                    } else if !form_xpub.value.is_empty() && form_xpub.valid {
                        Column::new()
                            .spacing(5)
                            .push(
                                Row::new()
                                    .spacing(5)
                                    .push(text("Fingerprint alias:").bold())
                                    .push(tooltip(prompt::DEFINE_DESCRIPTOR_FINGERPRINT_TOOLTIP)),
                            )
                            .push(
                                form::Form::new("Alias", form_name, |msg| {
                                    Message::DefineDescriptor(message::DefineDescriptor::KeyModal(
                                        message::ImportKeyModal::NameEdited(msg),
                                    ))
                                })
                                .warning("Please enter correct alias")
                                .size(text::P1_SIZE)
                                .padding(10),
                            )
                    } else {
                        Column::new()
                    },
                )
                .push_maybe(
                    if duplicate_master_fg {
                        Some(text("A single signing device may not be used more than once per path. (It can still be used in other paths.)").style(color::RED))
                    } else {
                        None
                    }
                )
                .push(
                    if form_xpub.valid && !form_xpub.value.is_empty() && !form_name.value.is_empty() && !duplicate_master_fg
                    {
                        button::primary(None, "Apply")
                            .on_press(Message::DefineDescriptor(
                                message::DefineDescriptor::KeyModal(
                                    message::ImportKeyModal::ConfirmXpub,
                                ),
                            ))
                            .width(Length::Fixed(200.0))
                    } else {
                        button::primary(None, "Apply").width(Length::Fixed(100.0))
                    },
                )
                .align_items(Alignment::Center),
        ))
        .width(Length::Fixed(600.0))
        .into()
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
        .align_items(Alignment::Center)
        .push(text("Activate recovery path after:"))
        .push(
            Row::new()
                .push(
                    Container::new(
                        form::Form::new_trimmed("ex: 1000", sequence, |v| {
                            Message::DefineDescriptor(message::DefineDescriptor::SequenceModal(
                                message::SequenceModal::SequenceEdited(v),
                            ))
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
                            Message::DefineDescriptor(message::DefineDescriptor::SequenceModal(
                                message::SequenceModal::SequenceEdited(v.to_string()),
                            ))
                        })
                        .step(144_u16), // 144 blocks per day
                    )
                    .width(Length::Fixed(500.0)),
                );
        }
    }

    card::simple(col.push(if sequence.valid {
        button::primary(None, "Apply")
            .on_press(Message::DefineDescriptor(
                message::DefineDescriptor::SequenceModal(message::SequenceModal::ConfirmSequence),
            ))
            .width(Length::Fixed(200.0))
    } else {
        button::primary(None, "Apply").width(Length::Fixed(200.0))
    }))
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
                    .style(theme::Button::Transparent)
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
                .align_items(Alignment::Center)
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

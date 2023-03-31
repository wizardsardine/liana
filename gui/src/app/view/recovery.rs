use std::collections::HashMap;

use iced::{
    widget::{tooltip, Space},
    Alignment, Length,
};

use liana::miniscript::bitcoin::{
    util::bip32::{DerivationPath, Fingerprint},
    Amount,
};

use liana_ui::{
    component::{button, form, text::*},
    icon, theme,
    widget::*,
};

use crate::app::view::{
    message::{CreateSpendMessage, Message},
    util::amount,
};

#[allow(clippy::too_many_arguments)]
pub fn recovery<'a>(
    recovery_paths: Vec<Element<'a, Message>>,
    selected_path: Option<usize>,
    feerate: &form::Value<String>,
    address: &'a form::Value<String>,
) -> Element<'a, Message> {
    Column::new()
        .push(Space::with_height(Length::Units(100)))
        .push(
            Row::new()
                .push(Container::new(
                    icon::recovery_icon().width(Length::Units(100)).size(50),
                ))
                .push(text("Recover the funds").size(50).bold())
                .align_items(Alignment::Center)
                .spacing(1),
        )
        .push(
            Container::new(
                Column::new()
                    .spacing(10)
                    .push(text(format!(
                        "{} recovery paths are available or will be available next block, select one:",
                        recovery_paths.len()
                    )))
                    .push(Column::with_children(recovery_paths).spacing(10)),
            )
            .padding(20),
        )
        .push(Space::with_height(Length::Units(20)))
        .push(
            Column::new()
                .push(text("Enter destination address and feerate:").bold())
                .push(
                    Container::new(
                        form::Form::new("Address", address, move |msg| {
                            Message::CreateSpend(CreateSpendMessage::RecipientEdited(
                                0, "address", msg,
                            ))
                        })
                        .warning("Invalid Bitcoin address")
                        .size(20)
                        .padding(10),
                    )
                    .width(Length::Units(250)),
                )
                .push(
                    Container::new(
                        form::Form::new("Feerate (sat/vbyte)", feerate, move |msg| {
                            Message::CreateSpend(CreateSpendMessage::FeerateEdited(msg))
                        })
                        .warning("Invalid feerate")
                        .size(20)
                        .padding(10),
                    )
                    .width(Length::Units(250)),
                )
                .push(
                    if feerate.valid
                        && !feerate.value.is_empty()
                        && address.valid
                        && !address.value.is_empty()
                        && selected_path.is_some()
                    {
                        button::primary(None, "Next")
                            .on_press(Message::Next)
                            .width(Length::Units(200))
                    } else {
                        button::primary(None, "Next").width(Length::Units(200))
                    },
                )
                .spacing(20)
                .align_items(Alignment::Center),
        )
        .align_items(Alignment::Center)
        .spacing(20)
        .into()
}

pub fn recovery_path_view<'a>(
    index: usize,
    threshold: usize,
    origins: &'a [(Fingerprint, DerivationPath)],
    total_amount: Amount,
    number_of_coins: usize,
    key_aliases: &'a HashMap<Fingerprint, String>,
    selected: bool,
) -> Element<'a, Message> {
    Container::new(
        Button::new(
            Row::new()
                .push(if selected {
                    icon::square_check_icon()
                } else {
                    icon::square_icon()
                })
                .push(
                    Column::new()
                        .push(
                            Row::new()
                                .align_items(Alignment::Center)
                                .spacing(10)
                                .push(
                                    text(format!(
                                        "{} signature{} from",
                                        threshold,
                                        if threshold > 1 { "s" } else { "" }
                                    ))
                                    .bold(),
                                )
                                .push(origins.iter().fold(
                                    Row::new().align_items(Alignment::Center).spacing(5),
                                    |row, (fg, _)| {
                                        row.push(if let Some(alias) = key_aliases.get(fg) {
                                            Container::new(
                                                tooltip::Tooltip::new(
                                                    Container::new(text(alias)).padding(3).style(
                                                        theme::Container::Pill(theme::Pill::Simple),
                                                    ),
                                                    fg.to_string(),
                                                    tooltip::Position::Bottom,
                                                )
                                                .style(theme::Container::Card(theme::Card::Simple)),
                                            )
                                        } else {
                                            Container::new(text(fg.to_string()))
                                                .padding(3)
                                                .style(theme::Container::Pill(theme::Pill::Simple))
                                        })
                                    },
                                )),
                        )
                        .push(
                            Row::new()
                                .spacing(5)
                                .push(text("can recover"))
                                .push(text(format!(
                                    "{} coin{} totalling",
                                    number_of_coins,
                                    if number_of_coins > 0 { "s" } else { "" }
                                )))
                                .push(amount(&total_amount)),
                        ),
                )
                .align_items(Alignment::Center)
                .spacing(20),
        )
        .padding(10)
        .width(Length::Fill)
        .on_press(Message::CreateSpend(CreateSpendMessage::SelectPath(index)))
        .style(theme::Button::TransparentBorder),
    )
    .style(theme::Container::Card(theme::Card::Simple))
    .width(Length::Fill)
    .into()
}

use std::collections::HashMap;

use iced::{
    widget::{checkbox, tooltip, Space},
    Alignment, Length,
};

use liana::miniscript::bitcoin::{
    bip32::{DerivationPath, Fingerprint},
    Amount,
};

use liana_ui::{
    component::{amount::*, button, form, text::*},
    icon, theme,
    util::*,
    widget::*,
};

use crate::app::{
    cache::Cache,
    menu::Menu,
    view::{
        dashboard,
        message::{CreateSpendMessage, Message},
    },
    Error,
};

#[allow(clippy::too_many_arguments)]
pub fn recovery<'a>(
    cache: &'a Cache,
    recovery_paths: Vec<Element<'a, Message>>,
    selected_path: Option<usize>,
    feerate: &form::Value<String>,
    address: &'a form::Value<String>,
    warning: Option<&Error>,
) -> Element<'a, Message> {
    let no_recovery_paths = recovery_paths.is_empty();
    dashboard(
        &Menu::Settings,
        cache,
        warning,
        Column::new()
            .push(
                Row::new()
                    .spacing(10)
                    .align_items(Alignment::Center)
                    .push(
                        Button::new(text("Settings").size(30).bold())
                            .style(theme::Button::Transparent)
                            .on_press(Message::Menu(Menu::Settings)),
                    )
                    .push(icon::chevron_right().size(30))
                    .push(
                        Button::new(text("Recovery").size(30).bold())
                            .style(theme::Button::Transparent)
                            .on_press(Message::Menu(Menu::Recovery)),
                    ),
            )
            .push(Space::with_height(Length::Fixed(20.0)))
            .push(
                Row::new()
                    .spacing(20)
                    .align_items(Alignment::Center)
                    .push(text("Destination").bold())
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
                        .max_width(500)
                        .width(Length::Fill),
                    )
                    .push(text("Feerate").bold())
                    .push(
                        Container::new(
                            form::Form::new("42 (sats/vbyte)", feerate, move |msg| {
                                Message::CreateSpend(CreateSpendMessage::FeerateEdited(msg))
                            })
                            .warning("Invalid feerate")
                            .size(20)
                            .padding(10),
                        )
                        .width(Length::Fixed(200.0)),
                    ),
            )
            .push(if no_recovery_paths {
                Container::new(text("No recovery path is currently available"))
            } else {
                Container::new(
                    Column::new()
                        .spacing(20)
                        .push(text(format!(
                            "{} recovery paths will be available at the next block, select one:",
                            recovery_paths.len()
                        )))
                        .push(Column::with_children(recovery_paths).spacing(20)),
                )
                .style(theme::Container::Card(theme::Card::Simple))
                .padding(20)
            })
            .push_maybe(if no_recovery_paths {
                None
            } else {
                Some(
                    Row::new()
                        .push(Space::with_width(Length::Fill))
                        .push(
                            if feerate.valid
                                && !feerate.value.is_empty()
                                && address.valid
                                && !address.value.is_empty()
                                && selected_path.is_some()
                            {
                                button::primary(None, "Next")
                                    .on_press(Message::Next)
                                    .width(Length::Fixed(200.0))
                            } else {
                                button::primary(None, "Next").width(Length::Fixed(200.0))
                            },
                        )
                        .spacing(20)
                        .align_items(Alignment::Center),
                )
            })
            .spacing(20),
    )
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
    Row::new()
        .push(checkbox("", selected, move |_| {
            Message::CreateSpend(CreateSpendMessage::SelectPath(index))
        }))
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
                                            Container::new(text(alias))
                                                .padding(5)
                                                .style(theme::Container::Pill(theme::Pill::Simple)),
                                            fg.to_string(),
                                            tooltip::Position::Bottom,
                                        )
                                        .style(theme::Container::Card(theme::Card::Simple)),
                                    )
                                } else {
                                    Container::new(text(fg.to_string()))
                                        .padding(5)
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
                )
                .spacing(5),
        )
        .width(Length::Fill)
        .align_items(Alignment::Center)
        .spacing(20)
        .into()
}

use std::collections::{HashMap, HashSet};

use iced::{
    widget::{checkbox, tooltip, Space},
    Alignment, Length,
};

use liana::miniscript::bitcoin::{
    bip32::{DerivationPath, Fingerprint},
    Amount,
};

use liana_ui::{
    component::{amount::*, button, text::*},
    theme,
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
    warning: Option<&Error>,
) -> Element<'a, Message> {
    let no_recovery_paths = recovery_paths.is_empty();
    dashboard(
        &Menu::Recovery,
        cache,
        warning,
        Column::new()
            .push(Container::new(h3("Recovery")).width(Length::Fill))
            .push(Space::with_height(Length::Fixed(20.0)))
            .push(if no_recovery_paths {
                Container::new(text("No recovery path is currently available"))
            } else {
                Container::new(
                    Column::new()
                        .spacing(20)
                        .push(text(format!(
                            "{} recovery path{} available:",
                            recovery_paths.len(),
                            if recovery_paths.len() > 1 {
                                "s are"
                            } else {
                                " is"
                            },
                        )))
                        .push(Column::with_children(recovery_paths).spacing(20)),
                )
                .style(theme::card::simple)
                .padding(20)
            })
            .push_maybe(if no_recovery_paths {
                None
            } else {
                Some(
                    Row::new()
                        .push(Space::with_width(Length::Fill))
                        .push(
                            button::secondary(None, "Next")
                                .on_press_maybe(selected_path.map(|_| Message::Next))
                                .width(Length::Fixed(200.0)),
                        )
                        .spacing(20)
                        .align_y(Alignment::Center),
                )
            })
            .spacing(20),
    )
}

pub fn recovery_path_view<'a>(
    index: usize,
    threshold: usize,
    origins: &'a [(Fingerprint, HashSet<DerivationPath>)],
    total_amount: Amount,
    number_of_coins: usize,
    key_aliases: &'a HashMap<Fingerprint, String>,
    selected: bool,
) -> Element<'a, Message> {
    Row::new()
        .push(
            checkbox("", selected)
                .on_toggle(move |_| Message::CreateSpend(CreateSpendMessage::SelectPath(index))),
        )
        .push(
            Column::new()
                .push(
                    Row::new()
                        .align_y(Alignment::Center)
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
                            Row::new().align_y(Alignment::Center).spacing(5),
                            |row, (fg, _)| {
                                row.push(if let Some(alias) = key_aliases.get(fg) {
                                    Container::new(
                                        tooltip::Tooltip::new(
                                            Container::new(text(alias))
                                                .padding(5)
                                                .style(theme::pill::simple),
                                            liana_ui::widget::Text::new(fg.to_string()),
                                            tooltip::Position::Top,
                                        )
                                        .style(theme::card::simple),
                                    )
                                } else {
                                    Container::new(text(fg.to_string()))
                                        .padding(5)
                                        .style(theme::pill::simple)
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
        .align_y(Alignment::Center)
        .spacing(20)
        .into()
}

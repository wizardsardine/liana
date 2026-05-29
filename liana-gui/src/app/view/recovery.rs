use std::collections::{HashMap, HashSet};

use iced::{
    widget::{checkbox, Space},
    Alignment, Length,
};

use liana::miniscript::bitcoin::{
    bip32::{DerivationPath, Fingerprint},
    Amount,
};

use liana_ui::{
    component::{amount::*, button, pill, text::*},
    theme,
    widget::*,
};

use crate::{
    app::{
        cache::Cache,
        menu::Menu,
        view::{
            dashboard,
            message::{CreateSpendMessage, Message},
        },
        Error,
    },
    t,
};

#[allow(clippy::too_many_arguments)]
pub fn recovery<'a>(
    cache: &'a Cache,
    recovery_paths: Vec<Element<'a, Message>>,
    selected_path: Option<usize>,
    warning: Option<&'a Error>,
) -> Element<'a, Message> {
    let no_recovery_paths = recovery_paths.is_empty();
    dashboard(
        &Menu::Recovery,
        cache,
        warning,
        Column::new()
            .push(Container::new(panel_title(Menu::Recovery.title())).width(Length::Fill))
            .push(Container::new(text(t!("recovery-info"))))
            .push(Space::with_height(Length::Fixed(20.0)))
            .push(
                Container::new(
                    Column::new()
                        .push(
                            text(if no_recovery_paths {
                                t!("recovery-none-available")
                            } else {
                                t!("recovery-paths-available", count = recovery_paths.len())
                            })
                            .width(Length::Fill),
                        )
                        .push_maybe((!no_recovery_paths).then_some(Space::with_height(20)))
                        .push(Column::with_children(recovery_paths).spacing(20)),
                )
                .style(theme::card::simple)
                .padding(20),
            )
            .push_maybe(if no_recovery_paths {
                None
            } else {
                Some(
                    Row::new()
                        .push(Space::with_width(Length::Fill))
                        .push(
                            button::primary(None, t!("common-next"))
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
            checkbox(selected)
                .on_toggle(move |_| Message::CreateSpend(CreateSpendMessage::SelectPath(index))),
        )
        .push(
            Column::new()
                .push(
                    Row::new()
                        .align_y(Alignment::Center)
                        .spacing(10)
                        .push(text(t!("recovery-signatures-from", count = threshold)).bold())
                        .push(origins.iter().fold(
                            Row::new().align_y(Alignment::Center).spacing(5),
                            |row, (fg, _)| {
                                row.push(pill::fingerprint(
                                    fg.to_string(),
                                    key_aliases.get(fg).map(String::as_str),
                                ))
                            },
                        )),
                )
                .push(
                    Row::new()
                        .spacing(5)
                        .push(text(t!("recovery-can-recover")))
                        .push(text(format!(
                            "{}",
                            t!("recovery-coins-total", count = number_of_coins)
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

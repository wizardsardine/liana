use std::collections::{HashMap, HashSet};

use iced::{
    widget::{checkbox, column, row, Space},
    Alignment, Length,
};

use liana::miniscript::bitcoin::{
    bip32::{DerivationPath, Fingerprint},
    Amount,
};

use liana_ui::{
    component::{
        amount::*,
        button, pill,
        text::{legacy, Text},
    },
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

pub fn recovery<'a>(
    cache: &'a Cache,
    recovery_paths: Vec<Element<'a, Message>>,
    selected_path: Option<usize>,
    warning: Option<&'a Error>,
) -> Element<'a, Message> {
    let no_recovery_paths = recovery_paths.is_empty();
    let title = legacy::panel_title(Menu::Recovery.title());
    let info = legacy::text(t!("recovery-info"));
    let header = column![title, info].spacing(20);
    let paths_title = legacy::text(if no_recovery_paths {
        t!("recovery-none-available")
    } else {
        t!("recovery-paths-available", count = recovery_paths.len())
    })
    .width(Length::Fill);
    let paths_spacer = (!no_recovery_paths).then_some(Space::with_height(20));
    let paths = Column::with_children(recovery_paths).spacing(20);
    let paths = Container::new(column![paths_title, paths_spacer, paths])
        .style(theme::card::simple)
        .padding(20);
    let next = (!no_recovery_paths).then_some(
        row![
            Space::fill_width(),
            button::btn_next(selected_path.map(|_| Message::Next))
        ]
        .spacing(20)
        .align_y(Alignment::Center),
    );

    let content = column![header, Space::with_height(20), paths, next].spacing(20);

    dashboard(&Menu::Recovery, cache, warning, content)
}

pub fn recovery_path_entry<'a>(
    index: usize,
    threshold: usize,
    origins: &'a [(Fingerprint, HashSet<DerivationPath>)],
    total_amount: Amount,
    number_of_coins: usize,
    key_aliases: &'a HashMap<Fingerprint, String>,
    selected: bool,
) -> Element<'a, Message> {
    let select = checkbox(selected)
        .on_toggle(move |_| Message::CreateSpend(CreateSpendMessage::SelectPath(index)));
    let keys = origins.iter().fold(
        Row::new().align_y(Alignment::Center).spacing(5),
        |row, (fg, _)| {
            row.push(pill::fingerprint(
                fg.to_string(),
                key_aliases.get(fg).map(String::as_str),
            ))
        },
    );
    let signatures = row![
        legacy::text(t!("recovery-signatures-from", count = threshold)).bold(),
        keys
    ]
    .align_y(Alignment::Center)
    .spacing(10);
    let coins = row![
        legacy::text(t!("recovery-coins-total", count = number_of_coins)),
        amount(&total_amount)
    ]
    .spacing(5);
    let description = column![signatures, coins].spacing(5);

    row![select, description]
        .width(Length::Fill)
        .align_y(Alignment::Center)
        .spacing(20)
        .into()
}

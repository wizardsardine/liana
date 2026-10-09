use std::collections::{HashMap, HashSet};

use iced::{
    widget::{column, row, Space},
    Length,
};

use liana::miniscript::bitcoin::{
    bip32::{DerivationPath, Fingerprint},
    Amount,
};

use liana_ui::{
    component::{button, panels::recovery, text::new},
    spacing::VSpacing,
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
    let title = new::d2(Menu::Recovery.title());
    let info = new::b2(t!("recovery-info")).style(theme::text::secondary);
    let header = column![title, info].spacing(VSpacing::L);

    let paths_title = new::d3(if no_recovery_paths {
        t!("recovery-none-available")
    } else {
        t!("recovery-paths-available", count = recovery_paths.len())
    })
    .width(Length::Fill);
    let paths_spacer = (!no_recovery_paths).then_some(Space::with_height(VSpacing::M));
    let paths = Column::with_children(recovery_paths).spacing(VSpacing::M);
    let paths = column![paths_title, paths_spacer, paths];
    let next = (!no_recovery_paths).then_some(row![
        Space::fill_width(),
        button::btn_next(selected_path.map(|_| Message::Next))
    ]);

    let content = column![
        header,
        Space::with_height(VSpacing::XXXL),
        paths,
        Space::with_height(VSpacing::L),
        next
    ];

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
    recovery::path_entry(
        threshold,
        origins,
        total_amount,
        number_of_coins,
        key_aliases,
        selected,
        Message::CreateSpend(CreateSpendMessage::SelectPath(index)),
    )
}

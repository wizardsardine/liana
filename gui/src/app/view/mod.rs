mod message;
mod util;
mod warning;

pub mod coins;
pub mod home;
pub mod hw;
pub mod psbts;
pub mod receive;
pub mod recovery;
pub mod settings;
pub mod spend;
pub mod transactions;

pub use message::*;
use warning::warn;

use iced::{
    widget::{column, row, scrollable, Space},
    Length,
};

use liana_ui::{
    component::{button, text::*},
    icon::{coin_icon, cross_icon, home_icon, receive_icon, send_icon, settings_icon},
    image::*,
    theme,
    util::Collection,
    widget::*,
};

use crate::app::{cache::Cache, error::Error, menu::Menu};

pub fn sidebar<'a>(menu: &Menu, cache: &'a Cache) -> Container<'a, Message> {
    let home_button = if *menu == Menu::Home {
        button::menu_active(Some(home_icon()), "Home")
            .on_press(Message::Reload)
            .width(iced::Length::Fill)
    } else {
        button::menu(Some(home_icon()), "Home")
            .on_press(Message::Menu(Menu::Home))
            .width(iced::Length::Fill)
    };

    let transactions_button = if *menu == Menu::Transactions {
        Button::new(
            row!(
                history_icon().width(Length::Units(20)),
                text("Transactions")
            )
            .spacing(10)
            .padding(10)
            .align_items(iced::Alignment::Center),
        )
        .style(theme::Button::Menu(true))
        .on_press(Message::Reload)
        .width(iced::Length::Fill)
    } else {
        Button::new(
            row!(
                history_icon().width(Length::Units(20)),
                text("Transactions")
            )
            .spacing(10)
            .padding(10)
            .align_items(iced::Alignment::Center),
        )
        .style(theme::Button::Menu(false))
        .on_press(Message::Menu(Menu::Transactions))
        .width(iced::Length::Fill)
    };

    let coins_button = if *menu == Menu::Coins {
        Button::new(
            Container::new(
                Row::new()
                    .push(
                        Row::new()
                            .push(coin_icon())
                            .push(text("Coins"))
                            .spacing(10)
                            .width(iced::Length::Fill)
                            .align_items(iced::Alignment::Center),
                    )
                    .push(
                        Container::new(
                            text(format!(
                                "  {}  ",
                                cache
                                    .coins
                                    .iter()
                                    // TODO: Remove when cache contains only current coins.
                                    .filter(|coin| coin.spend_info.is_none())
                                    .count()
                            ))
                            .small()
                            .bold(),
                        )
                        .style(theme::Container::Pill(theme::Pill::Primary)),
                    )
                    .spacing(10)
                    .width(iced::Length::Fill)
                    .align_items(iced::Alignment::Center),
            )
            .width(iced::Length::Fill)
            .padding(10)
            .center_x(),
        )
        .style(theme::Button::Menu(true))
        .on_press(Message::Reload)
        .width(iced::Length::Fill)
    } else {
        Button::new(
            Container::new(
                Row::new()
                    .push(
                        Row::new()
                            .push(coin_icon())
                            .push(text("Coins"))
                            .spacing(10)
                            .width(iced::Length::Fill)
                            .align_items(iced::Alignment::Center),
                    )
                    .push(
                        Container::new(
                            text(format!(
                                "  {}  ",
                                cache
                                    .coins
                                    .iter()
                                    // TODO: Remove when cache contains only current coins.
                                    .filter(|coin| coin.spend_info.is_none())
                                    .count()
                            ))
                            .small()
                            .bold(),
                        )
                        .style(theme::Pill::Primary),
                    )
                    .spacing(10)
                    .width(iced::Length::Fill)
                    .align_items(iced::Alignment::Center),
            )
            .width(iced::Length::Fill)
            .padding(10)
            .center_x(),
        )
        .style(theme::Button::Menu(false))
        .on_press(Message::Menu(Menu::Coins))
        .width(iced::Length::Fill)
    };

    let psbt_button = if *menu == Menu::PSBTs {
        Button::new(
            Container::new(
                Row::new()
                    .push(
                        Row::new()
                            .push(history_icon().width(Length::Units(20)))
                            .push(text("PSBTs"))
                            .spacing(10)
                            .width(iced::Length::Fill)
                            .align_items(iced::Alignment::Center),
                    )
                    .push_maybe(if cache.spend_txs.is_empty() {
                        None
                    } else {
                        Some(
                            Container::new(
                                text(format!("  {}  ", cache.spend_txs.len()))
                                    .small()
                                    .bold(),
                            )
                            .style(theme::Pill::Primary),
                        )
                    })
                    .spacing(10)
                    .width(iced::Length::Fill)
                    .align_items(iced::Alignment::Center),
            )
            .width(iced::Length::Fill)
            .padding(10)
            .center_x(),
        )
        .style(theme::Button::Menu(true))
        .on_press(Message::Reload)
        .width(iced::Length::Fill)
    } else {
        Button::new(
            Container::new(
                Row::new()
                    .push(
                        Row::new()
                            .push(history_icon().width(Length::Units(20)))
                            .push(text("PSBTs"))
                            .spacing(10)
                            .width(iced::Length::Fill)
                            .align_items(iced::Alignment::Center),
                    )
                    .push_maybe(if cache.spend_txs.is_empty() {
                        None
                    } else {
                        Some(
                            Container::new(
                                text(format!("  {}  ", cache.spend_txs.len()))
                                    .small()
                                    .bold(),
                            )
                            .style(theme::Pill::Primary),
                        )
                    })
                    .spacing(10)
                    .width(iced::Length::Fill)
                    .align_items(iced::Alignment::Center),
            )
            .width(iced::Length::Fill)
            .padding(10)
            .center_x(),
        )
        .style(theme::Button::Menu(false))
        .on_press(Message::Menu(Menu::PSBTs))
        .width(iced::Length::Fill)
    };

    let spend_button = if *menu == Menu::CreateSpendTx {
        Button::new(
            Container::new(
                Row::new()
                    .push(send_icon())
                    .push(text("Send"))
                    .spacing(10)
                    .width(iced::Length::Fill)
                    .align_items(iced::Alignment::Center),
            )
            .width(iced::Length::Fill)
            .padding(10)
            .center_x(),
        )
        .style(theme::Button::Menu(true))
        .on_press(Message::Reload)
        .width(iced::Length::Fill)
    } else {
        Button::new(
            Container::new(
                Row::new()
                    .push(send_icon())
                    .push(text("Send"))
                    .spacing(10)
                    .width(iced::Length::Fill)
                    .align_items(iced::Alignment::Center),
            )
            .width(iced::Length::Fill)
            .padding(10)
            .center_x(),
        )
        .style(theme::Button::Menu(false))
        .on_press(Message::Menu(Menu::CreateSpendTx))
        .width(iced::Length::Fill)
    };

    let receive_button = if *menu == Menu::Receive {
        button::menu_active(Some(receive_icon()), "Receive")
            .on_press(Message::Reload)
            .width(iced::Length::Fill)
    } else {
        button::menu(Some(receive_icon()), "Receive")
            .on_press(Message::Menu(Menu::Receive))
            .width(iced::Length::Fill)
    };

    let settings_button = if *menu == Menu::Settings {
        button::menu_active(Some(settings_icon()), "Settings")
            .on_press(Message::Menu(Menu::Settings))
            .width(iced::Length::Fill)
    } else {
        button::menu(Some(settings_icon()), "Settings")
            .on_press(Message::Menu(Menu::Settings))
            .width(iced::Length::Fill)
    };

    Container::new(
        Column::new()
            .push(
                Column::new()
                    .push(
                        Container::new(
                            liana_grey_logo()
                                .height(Length::Units(150))
                                .width(Length::Units(60)),
                        )
                        .padding(15),
                    )
                    .push(home_button)
                    .push(spend_button)
                    .push(receive_button)
                    .push(coins_button)
                    .push(psbt_button)
                    .push(transactions_button)
                    .spacing(15)
                    .height(Length::Fill),
            )
            .push(
                Container::new(
                    Column::new()
                        .spacing(10)
                        .push_maybe(cache.rescan_progress.map(|p| {
                            Container::new(text(format!("  Rescan...{:.2}%  ", p * 100.0)))
                                .padding(5)
                                .style(theme::Pill::Simple)
                        }))
                        .push(settings_button),
                )
                .height(Length::Shrink),
            ),
    )
    .style(theme::Container::Foreground)
}

pub fn dashboard<'a, T: Into<Element<'a, Message>>>(
    menu: &'a Menu,
    cache: &'a Cache,
    warning: Option<&Error>,
    content: T,
) -> Element<'a, Message> {
    Row::new()
        .push(
            sidebar(menu, cache)
                .width(Length::FillPortion(2))
                .height(Length::Fill),
        )
        .push(
            Column::new()
                .push(warn(warning))
                .push(
                    main_section(Container::new(scrollable(row!(
                        Space::with_width(Length::FillPortion(1)),
                        column!(Space::with_height(Length::Units(150)), content.into())
                            .width(Length::FillPortion(8)),
                        Space::with_width(Length::FillPortion(1)),
                    ))))
                    .width(Length::Fill),
                )
                .width(Length::FillPortion(10)),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn main_section<'a, T: 'a>(menu: Container<'a, T>) -> Container<'a, T> {
    Container::new(menu.max_width(1500))
        .style(theme::Container::Background)
        .center_x()
        .width(Length::Fill)
        .height(Length::Fill)
}

pub fn modal<'a, T: Into<Element<'a, Message>>, F: Into<Element<'a, Message>>>(
    is_previous: bool,
    warning: Option<&Error>,
    content: T,
    fixed_footer: Option<F>,
) -> Element<'a, Message> {
    Column::new()
        .push(warn(warning))
        .push(
            Container::new(
                Row::new()
                    .push(if is_previous {
                        Column::new()
                            .push(
                                button::transparent(None, "< Previous").on_press(Message::Previous),
                            )
                            .width(Length::Fill)
                    } else {
                        Column::new().width(Length::Fill)
                    })
                    .align_items(iced::Alignment::Center)
                    .push(button::primary(Some(cross_icon()), "Close").on_press(Message::Close)),
            )
            .padding(10)
            .style(theme::Container::Background),
        )
        .push(modal_section(Container::new(scrollable(content))))
        .push_maybe(fixed_footer)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn modal_section<'a, T: 'a>(menu: Container<'a, T>) -> Container<'a, T> {
    Container::new(menu.max_width(1500))
        .style(theme::Container::Background)
        .center_x()
        .width(Length::Fill)
        .height(Length::Fill)
}

mod label;
mod message;
mod warning;

pub mod coins;
pub mod home;
pub mod hw;
pub mod psbt;
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
    color,
    component::{button, text::*},
    icon::{
        coins_icon, cross_icon, history_icon, home_icon, receive_icon, send_icon, settings_icon,
    },
    image::*,
    theme,
    util::Collection,
    widget::*,
};

use crate::app::{cache::Cache, error::Error, menu::Menu};

fn menu_green_bar<'a, T: 'a>() -> Container<'a, T> {
    Container::new(Space::with_width(Length::Fixed(2.0)))
        .height(Length::Fixed(50.0))
        .style(theme::Container::Custom(color::GREEN))
}

pub fn sidebar<'a>(menu: &Menu, cache: &'a Cache) -> Container<'a, Message> {
    let home_button = if *menu == Menu::Home {
        row!(
            button::menu_active(Some(home_icon()), "Home")
                .on_press(Message::Reload)
                .width(iced::Length::Fill),
            menu_green_bar(),
        )
    } else {
        row!(button::menu(Some(home_icon()), "Home")
            .on_press(Message::Menu(Menu::Home))
            .width(iced::Length::Fill),)
    };

    let transactions_button = if *menu == Menu::Transactions {
        row!(
            button::menu_active(Some(history_icon()), "Transactions")
                .on_press(Message::Reload)
                .width(iced::Length::Fill),
            menu_green_bar()
        )
    } else {
        row!(button::menu(Some(history_icon()), "Transactions")
            .on_press(Message::Menu(Menu::Transactions))
            .width(iced::Length::Fill))
    };

    let coins_button = if *menu == Menu::Coins {
        row!(
            button::menu_active(Some(coins_icon()), "Coins")
                .on_press(Message::Reload)
                .width(iced::Length::Fill),
            menu_green_bar()
        )
    } else {
        row!(button::menu(Some(coins_icon()), "Coins")
            .style(theme::Button::Menu(false))
            .on_press(Message::Menu(Menu::Coins))
            .width(iced::Length::Fill))
    };

    let psbt_button = if *menu == Menu::PSBTs {
        row!(
            button::menu_active(Some(history_icon()), "PSBTs")
                .on_press(Message::Reload)
                .width(iced::Length::Fill),
            menu_green_bar()
        )
    } else {
        row!(button::menu(Some(history_icon()), "PSBTs")
            .on_press(Message::Menu(Menu::PSBTs))
            .width(iced::Length::Fill))
    };

    let spend_button = if *menu == Menu::CreateSpendTx {
        row!(
            button::menu_active(Some(send_icon()), "Send")
                .on_press(Message::Menu(Menu::CreateSpendTx))
                .width(iced::Length::Fill),
            menu_green_bar()
        )
    } else {
        row!(button::menu(Some(send_icon()), "Send")
            .on_press(Message::Menu(Menu::CreateSpendTx))
            .width(iced::Length::Fill))
    };

    let receive_button = if *menu == Menu::Receive {
        row!(
            button::menu_active(Some(receive_icon()), "Receive")
                .on_press(Message::Reload)
                .width(iced::Length::Fill),
            menu_green_bar()
        )
    } else {
        row!(button::menu(Some(receive_icon()), "Receive")
            .on_press(Message::Menu(Menu::Receive))
            .width(iced::Length::Fill))
    };

    let settings_button = if *menu == Menu::Settings {
        row!(
            button::menu_active(Some(settings_icon()), "Settings")
                .on_press(Message::Menu(Menu::Settings))
                .width(iced::Length::Fill),
            menu_green_bar()
        )
    } else {
        row!(button::menu(Some(settings_icon()), "Settings")
            .on_press(Message::Menu(Menu::Settings))
            .width(iced::Length::Fill))
    };

    Container::new(
        Column::new()
            .push(
                Column::new()
                    .push(
                        Container::new(
                            liana_grey_logo()
                                .height(Length::Fixed(120.0))
                                .width(Length::Fixed(60.0)),
                        )
                        .padding(10),
                    )
                    .push(home_button)
                    .push(spend_button)
                    .push(receive_button)
                    .push(coins_button)
                    .push(transactions_button)
                    .push(psbt_button)
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
                    Container::new(scrollable(row!(
                        Space::with_width(Length::FillPortion(1)),
                        column!(Space::with_height(Length::Fixed(150.0)), content.into())
                            .width(Length::FillPortion(8))
                            .max_width(1500),
                        Space::with_width(Length::FillPortion(1)),
                    )))
                    .center_x()
                    .style(theme::Container::Background)
                    .width(Length::Fill)
                    .height(Length::Fill),
                )
                .width(Length::FillPortion(10)),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
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

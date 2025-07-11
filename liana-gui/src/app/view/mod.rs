mod label;
mod message;
mod warning;

pub mod coins;
pub mod export;
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
    widget::{column, responsive, row, scrollable, Space},
    Length,
};

use liana_ui::{
    color,
    component::{button, text::*},
    icon::{
        coins_icon, cross_icon, history_icon, home_icon, receive_icon, recovery_icon, send_icon,
        settings_icon,
    },
    image::*,
    theme,
    widget::*,
};

use crate::app::{cache::Cache, error::Error, menu::Menu};

fn menu_green_bar<'a, T: 'a>() -> Container<'a, T> {
    Container::new(Space::with_width(Length::Fixed(2.0)))
        .height(Length::Fixed(50.0))
        .style(theme::container::custom(color::GREEN))
}

pub fn sidebar<'a>(menu: &Menu, cache: &'a Cache) -> Container<'a, Message> {
    let home_button = if *menu == Menu::Home {
        row!(
            button::menu_active(Some(home_icon()), "Home")
                .on_press(Message::Reload(true))
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
                .on_press(Message::Reload(true))
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
                .on_press(Message::Reload(true))
                .width(iced::Length::Fill),
            menu_green_bar()
        )
    } else {
        row!(button::menu(Some(coins_icon()), "Coins")
            .style(theme::button::menu)
            .on_press(Message::Menu(Menu::Coins))
            .width(iced::Length::Fill))
    };

    let psbt_button = if *menu == Menu::PSBTs {
        row!(
            button::menu_active(Some(history_icon()), "PSBTs")
                .on_press(Message::Reload(true))
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
                .on_press(Message::Reload(true))
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
                .on_press(Message::Reload(true))
                .width(iced::Length::Fill),
            menu_green_bar()
        )
    } else {
        row!(button::menu(Some(receive_icon()), "Receive")
            .on_press(Message::Menu(Menu::Receive))
            .width(iced::Length::Fill))
    };

    let recovery_button = if *menu == Menu::Recovery {
        row!(
            button::menu_active(Some(recovery_icon()), "Recovery")
                .on_press(Message::Reload(true))
                .width(iced::Length::Fill),
            menu_green_bar()
        )
    } else {
        row!(button::menu(Some(recovery_icon()), "Recovery")
            .on_press(Message::Menu(Menu::Recovery))
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
                                .style(theme::pill::simple)
                        }))
                        .push(recovery_button)
                        .push(settings_button),
                )
                .height(Length::Shrink),
            ),
    )
    .style(theme::container::foreground)
}

pub fn small_sidebar<'a>(menu: &Menu, cache: &'a Cache) -> Container<'a, Message> {
    let home_button = if *menu == Menu::Home {
        row!(
            button::menu_active_small(home_icon())
                .on_press(Message::Reload(true))
                .width(iced::Length::Fill),
            menu_green_bar(),
        )
    } else {
        row!(button::menu_small(home_icon())
            .on_press(Message::Menu(Menu::Home))
            .width(iced::Length::Fill),)
    };

    let transactions_button = if *menu == Menu::Transactions {
        row!(
            button::menu_active_small(history_icon())
                .on_press(Message::Reload(true))
                .width(iced::Length::Fill),
            menu_green_bar()
        )
    } else {
        row!(button::menu_small(history_icon())
            .on_press(Message::Menu(Menu::Transactions))
            .width(iced::Length::Fill))
    };

    let coins_button = if *menu == Menu::Coins {
        row!(
            button::menu_active_small(coins_icon())
                .on_press(Message::Reload(true))
                .width(iced::Length::Fill),
            menu_green_bar()
        )
    } else {
        row!(button::menu_small(coins_icon())
            .style(theme::button::menu)
            .on_press(Message::Menu(Menu::Coins))
            .width(iced::Length::Fill))
    };

    let psbt_button = if *menu == Menu::PSBTs {
        row!(
            button::menu_active_small(history_icon())
                .on_press(Message::Reload(true))
                .width(iced::Length::Fill),
            menu_green_bar()
        )
    } else {
        row!(button::menu_small(history_icon())
            .on_press(Message::Menu(Menu::PSBTs))
            .width(iced::Length::Fill))
    };

    let spend_button = if *menu == Menu::CreateSpendTx {
        row!(
            button::menu_active_small(send_icon())
                .on_press(Message::Reload(true))
                .width(iced::Length::Fill),
            menu_green_bar()
        )
    } else {
        row!(button::menu_small(send_icon())
            .on_press(Message::Menu(Menu::CreateSpendTx))
            .width(iced::Length::Fill))
    };

    let receive_button = if *menu == Menu::Receive {
        row!(
            button::menu_active_small(receive_icon())
                .on_press(Message::Reload(true))
                .width(iced::Length::Fill),
            menu_green_bar()
        )
    } else {
        row!(button::menu_small(receive_icon())
            .on_press(Message::Menu(Menu::Receive))
            .width(iced::Length::Fill))
    };

    let recovery_button = if *menu == Menu::Recovery {
        row!(
            button::menu_active_small(recovery_icon())
                .on_press(Message::Reload(true))
                .width(iced::Length::Fill),
            menu_green_bar()
        )
    } else {
        row!(button::menu_small(recovery_icon())
            .on_press(Message::Menu(Menu::Recovery))
            .width(iced::Length::Fill))
    };

    let settings_button = if *menu == Menu::Settings {
        row!(
            button::menu_active_small(settings_icon())
                .on_press(Message::Menu(Menu::Settings))
                .width(iced::Length::Fill),
            menu_green_bar()
        )
    } else {
        row!(button::menu_small(settings_icon())
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
                    .align_x(iced::Alignment::Center)
                    .height(Length::Fill),
            )
            .push(
                Container::new(
                    Column::new()
                        .spacing(10)
                        .push_maybe(cache.rescan_progress.map(|p| {
                            Container::new(text(format!("{:.2}%  ", p * 100.0)))
                                .padding(5)
                                .style(theme::pill::simple)
                        }))
                        .push(recovery_button)
                        .push(settings_button),
                )
                .height(Length::Shrink),
            )
            .align_x(iced::Alignment::Center),
    )
    .style(theme::container::foreground)
}

pub fn dashboard<'a, T: Into<Element<'a, Message>>>(
    menu: &'a Menu,
    cache: &'a Cache,
    warning: Option<&Error>,
    content: T,
) -> Element<'a, Message> {
    Row::new()
        .push(
            Container::new(responsive(move |size| {
                if size.width > 150.0 {
                    sidebar(menu, cache).height(Length::Fill).into()
                } else {
                    small_sidebar(menu, cache).height(Length::Fill).into()
                }
            }))
            .width(Length::FillPortion(2)),
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
                    .center_x(Length::Fill)
                    .style(theme::container::background)
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
                    .align_y(iced::Alignment::Center)
                    .push(button::secondary(Some(cross_icon()), "Close").on_press(Message::Close)),
            )
            .padding(10)
            .style(theme::container::background),
        )
        .push(modal_section(Container::new(scrollable(content))))
        .push_maybe(fixed_footer)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn modal_section<'a, T: 'a>(menu: Container<'a, T>) -> Container<'a, T> {
    Container::new(menu.max_width(1500))
        .style(theme::container::background)
        .center_x(Length::Fill)
        .width(Length::Fill)
        .height(Length::Fill)
}

mod message;
mod warning;

pub mod coins;
pub mod home;
pub mod receive;
pub mod settings;
pub mod spend;

pub use message::*;
use warning::warn;

use iced::{
    pure::{column, container, row, scrollable, widget, Element},
    Length,
};

use crate::ui::{
    color,
    component::{badge, button, separation, text::*},
    icon::{coin_icon, cross_icon, home_icon, receive_icon, send_icon, settings_icon},
    util::Collection,
};

use crate::app::{cache::Cache, error::Error, menu::Menu};

pub fn sidebar<'a>(menu: &Menu, cache: &'a Cache) -> widget::Container<'a, Message> {
    let home_button = if *menu == Menu::Home {
        button::primary(Some(home_icon()), "Home")
            .on_press(Message::Reload)
            .width(iced::Length::Units(200))
    } else {
        button::transparent(Some(home_icon()), "Home")
            .on_press(Message::Menu(Menu::Home))
            .width(iced::Length::Units(200))
    };

    let coins_button = if *menu == Menu::Coins {
        iced::pure::widget::button::Button::new(
            container(
                row()
                    .push(
                        row()
                            .push(coin_icon())
                            .push(text("Coins"))
                            .spacing(10)
                            .width(iced::Length::Fill)
                            .align_items(iced::Alignment::Center),
                    )
                    .push(
                        container(
                            text(&format!(
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
                        .style(badge::PillStyle::InversePrimary),
                    )
                    .spacing(10)
                    .width(iced::Length::Fill)
                    .align_items(iced::Alignment::Center),
            )
            .width(iced::Length::Fill)
            .padding(5)
            .center_x(),
        )
        .style(button::Style::Primary)
        .on_press(Message::Reload)
        .width(iced::Length::Units(200))
    } else {
        iced::pure::widget::button::Button::new(
            container(
                row()
                    .push(
                        row()
                            .push(coin_icon())
                            .push(text("Coins"))
                            .spacing(10)
                            .width(iced::Length::Fill)
                            .align_items(iced::Alignment::Center),
                    )
                    .push(
                        container(
                            text(&format!(
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
                        .style(badge::PillStyle::Primary),
                    )
                    .spacing(10)
                    .width(iced::Length::Fill)
                    .align_items(iced::Alignment::Center),
            )
            .width(iced::Length::Fill)
            .padding(5)
            .center_x(),
        )
        .style(button::Style::Transparent)
        .on_press(Message::Menu(Menu::Coins))
        .width(iced::Length::Units(200))
    };

    let spend_button = if *menu == Menu::Spend {
        iced::pure::widget::button::Button::new(
            container(
                row()
                    .push(
                        row()
                            .push(send_icon())
                            .push(text("Send"))
                            .spacing(10)
                            .width(iced::Length::Fill)
                            .align_items(iced::Alignment::Center),
                    )
                    .push_maybe(if cache.spend_txs.is_empty() {
                        None
                    } else {
                        Some(
                            container(
                                text(&format!("  {}  ", cache.spend_txs.len()))
                                    .small()
                                    .bold(),
                            )
                            .style(badge::PillStyle::InversePrimary),
                        )
                    })
                    .spacing(10)
                    .width(iced::Length::Fill)
                    .align_items(iced::Alignment::Center),
            )
            .width(iced::Length::Fill)
            .padding(5)
            .center_x(),
        )
        .style(button::Style::Primary)
        .on_press(Message::Reload)
        .width(iced::Length::Units(200))
    } else {
        iced::pure::widget::button::Button::new(
            container(
                row()
                    .push(
                        row()
                            .push(send_icon())
                            .push(text("Send"))
                            .spacing(10)
                            .width(iced::Length::Fill)
                            .align_items(iced::Alignment::Center),
                    )
                    .push_maybe(if cache.spend_txs.is_empty() {
                        None
                    } else {
                        Some(
                            container(
                                text(&format!("  {}  ", cache.spend_txs.len()))
                                    .small()
                                    .bold(),
                            )
                            .style(badge::PillStyle::Primary),
                        )
                    })
                    .spacing(10)
                    .width(iced::Length::Fill)
                    .align_items(iced::Alignment::Center),
            )
            .width(iced::Length::Fill)
            .padding(5)
            .center_x(),
        )
        .style(button::Style::Transparent)
        .on_press(Message::Menu(Menu::Spend))
        .width(iced::Length::Units(200))
    };

    let receive_button = if *menu == Menu::Receive {
        button::primary(Some(receive_icon()), "Receive")
            .on_press(Message::Reload)
            .width(iced::Length::Units(200))
    } else {
        button::transparent(Some(receive_icon()), "Receive")
            .on_press(Message::Menu(Menu::Receive))
            .width(iced::Length::Units(200))
    };

    let settings_button = if *menu == Menu::Settings {
        button::primary(Some(settings_icon()), "Settings")
            .on_press(Message::Menu(Menu::Settings))
            .width(iced::Length::Units(200))
    } else {
        button::transparent(Some(settings_icon()), "Settings")
            .on_press(Message::Menu(Menu::Settings))
            .width(iced::Length::Units(200))
    };

    container(
        column()
            .padding(10)
            .push(
                column()
                    .push(
                        column()
                            .push(container(text("Minisafe").bold()).padding(10))
                            .push(separation().width(Length::Units(200)))
                            .spacing(10),
                    )
                    .push(home_button)
                    .push(coins_button)
                    .push(spend_button)
                    .push(receive_button)
                    .spacing(15)
                    .height(Length::Fill),
            )
            .push(container(settings_button).height(Length::Shrink)),
    )
    .style(SidebarStyle)
}

pub struct SidebarStyle;
impl widget::container::StyleSheet for SidebarStyle {
    fn style(&self) -> widget::container::Style {
        widget::container::Style {
            background: color::FOREGROUND.into(),
            border_width: 1.0,
            border_color: color::SECONDARY,
            ..widget::container::Style::default()
        }
    }
}

pub fn dashboard<'a, T: Into<Element<'a, Message>>>(
    menu: &'a Menu,
    cache: &'a Cache,
    warning: Option<&Error>,
    content: T,
) -> Element<'a, Message> {
    row()
        .push(
            sidebar(menu, cache)
                .width(Length::Shrink)
                .height(Length::Fill),
        )
        .push(
            column().push(warn(warning)).push(
                main_section(container(scrollable(content)))
                    .width(Length::Fill)
                    .height(Length::Fill),
            ),
        )
        .width(iced::Length::Fill)
        .height(iced::Length::Fill)
        .into()
}

fn main_section<'a, T: 'a>(menu: widget::Container<'a, T>) -> widget::Container<'a, T> {
    container(menu.max_width(1500))
        .padding(20)
        .style(MainSectionStyle)
        .center_x()
        .width(Length::Fill)
        .height(Length::Fill)
}

pub struct MainSectionStyle;
impl widget::container::StyleSheet for MainSectionStyle {
    fn style(&self) -> widget::container::Style {
        widget::container::Style {
            background: color::BACKGROUND.into(),
            ..widget::container::Style::default()
        }
    }
}

pub fn modal<'a, T: Into<Element<'a, Message>>>(
    is_previous: bool,
    warning: Option<&Error>,
    content: T,
) -> Element<'a, Message> {
    column()
        .push(warn(warning))
        .push(
            container(
                row()
                    .push(if is_previous {
                        column()
                            .push(
                                button::transparent(None, "< Previous").on_press(Message::Previous),
                            )
                            .width(Length::Fill)
                    } else {
                        column().width(Length::Fill)
                    })
                    .align_items(iced::Alignment::Center)
                    .push(button::primary(Some(cross_icon()), "Close").on_press(Message::Close)),
            )
            .padding(10)
            .style(ModalSectionStyle),
        )
        .push(modal_section(container(scrollable(content))))
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn modal_section<'a, T: 'a>(menu: widget::Container<'a, T>) -> widget::Container<'a, T> {
    container(menu.max_width(1500))
        .padding(20)
        .style(ModalSectionStyle)
        .center_x()
        .width(Length::Fill)
        .height(Length::Fill)
}

pub struct ModalSectionStyle;
impl widget::container::StyleSheet for ModalSectionStyle {
    fn style(&self) -> widget::container::Style {
        widget::container::Style {
            background: color::BACKGROUND.into(),
            ..widget::container::Style::default()
        }
    }
}

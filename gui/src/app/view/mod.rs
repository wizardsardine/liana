mod message;
mod warning;

pub mod settings;

pub use message::*;
use warning::warn;

use iced::{
    pure::{column, container, row, scrollable, widget, Element},
    Length,
};

use crate::ui::{
    color,
    component::{button, separation, text::*},
    icon::{home_icon, settings_icon},
};

use crate::app::{error::Error, menu::Menu};

pub fn sidebar(menu: &Menu) -> widget::Container<Message> {
    let home_button = if *menu == Menu::Home {
        button::primary(Some(home_icon()), "Home")
            .on_press(Message::Reload)
            .width(iced::Length::Units(200))
    } else {
        button::transparent(Some(home_icon()), "Home")
            .on_press(Message::Menu(Menu::Home))
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
                    .spacing(20)
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
    warning: Option<&Error>,
    content: T,
) -> Element<'a, Message> {
    row()
        .push(sidebar(menu).width(Length::Shrink).height(Length::Fill))
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

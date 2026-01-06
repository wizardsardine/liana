pub mod about;
pub mod general;

use iced::widget::{Column, Row};
use iced::{Alignment, Length};

use coincube_ui::component::{badge, text::*};
use coincube_ui::{icon, theme, widget::*};

use crate::app::cache::Cache;
use crate::app::menu::Menu;
use crate::app::view::{dashboard, message::*};

pub fn header(title: &str, msg: SettingsMessage) -> Element<'static, Message> {
    Row::new()
        .spacing(10)
        .align_y(Alignment::Center)
        .push(
            Button::new(text("Settings").size(30).bold())
                .style(theme::button::transparent)
                .on_press(Message::Menu(Menu::Settings(
                    crate::app::menu::SettingsSubMenu::General,
                ))),
        )
        .push(icon::chevron_right().size(30))
        .push(
            Button::new(text(title).size(30).bold())
                .style(theme::button::transparent)
                .on_press(Message::Settings(msg)),
        )
        .into()
}

fn settings_section(
    title: &str,
    icon: coincube_ui::widget::Text<'static>,
    msg: Message,
) -> Container<'static, Message> {
    Container::new(
        Button::new(
            Row::new()
                .push(badge::badge(icon))
                .push(text(title).bold())
                .padding(10)
                .spacing(20)
                .align_y(Alignment::Center)
                .width(Length::Fill),
        )
        .width(Length::Fill)
        .style(theme::button::transparent_border)
        .on_press(msg),
    )
    .width(Length::Fill)
    .style(theme::card::simple)
}

pub fn list<'a>(menu: &'a Menu, cache: &'a Cache) -> Element<'a, Message> {
    let header = Button::new(text("Settings").size(30).bold())
        .style(theme::button::transparent)
        .on_press(Message::Menu(Menu::Settings(
            crate::app::menu::SettingsSubMenu::General,
        )));

    dashboard(
        menu,
        cache,
        None,
        Column::new()
            .spacing(20)
            .push(header)
            .push(settings_section(
                "General",
                icon::wrench_icon(),
                Message::Settings(SettingsMessage::GeneralSection),
            ))
            .push(settings_section(
                "About",
                icon::tooltip_icon(),
                Message::Settings(SettingsMessage::AboutSection),
            )),
    )
}

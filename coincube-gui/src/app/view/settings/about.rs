use iced::widget::{Column, Row, Space};
use iced::{Alignment, Length};

use coincube_ui::component::{badge, card, separation, text::*};
use coincube_ui::{icon, widget::*};

use crate::app::cache;
use crate::app::error::Error;
use crate::app::menu::Menu;
use crate::app::view::dashboard;
use crate::app::view::message::{Message, SettingsMessage};

pub fn about_section<'a>(
    menu: &'a Menu,
    cache: &'a cache::Cache,
    warning: Option<&Error>,
    coincubed_version: Option<&String>,
) -> Element<'a, Message> {
    let content = card::simple(
        Column::new()
            .push(
                Row::new()
                    .push(badge::badge(icon::tooltip_icon()))
                    .push(text("Version").bold())
                    .padding(10)
                    .spacing(20)
                    .align_y(Alignment::Center)
                    .width(Length::Fill),
            )
            .push(separation().width(Length::Fill))
            .push(Space::new().height(Length::Fixed(10.0)))
            .push(
                Row::new().push(Space::new().width(Length::Fill)).push(
                    Column::new()
                        .push(text(format!("coincube-gui v{}", crate::VERSION)))
                        .push_maybe(
                            coincubed_version
                                .map(|version| text(format!("coincubed v{}", version))),
                        ),
                ),
            ),
    );

    dashboard(
        menu,
        cache,
        warning,
        Column::new()
            .spacing(20)
            .push(super::header("About", SettingsMessage::AboutSection))
            .push(content)
            .width(Length::Fill),
    )
}

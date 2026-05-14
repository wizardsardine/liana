use iced::widget::{Column, Row, Space};
use iced::{Alignment, Length};

use coincube_ui::component::{badge, button, card, separation, text::*};
use coincube_ui::{
    icon,
    widget::{ColumnExt, Element},
};

use crate::app::cache;
use crate::app::menu::Menu;
use crate::app::state::settings::about::AboutSettingsState;
use crate::app::view::dashboard;
use crate::app::view::message::{Message, SettingsMessage};

pub fn about_section<'a>(
    menu: &'a Menu,
    cache: &'a cache::Cache,
    coincubed_version: Option<&String>,
    state: &'a AboutSettingsState,
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

    let mut col = Column::new()
        .spacing(20)
        .push(super::header("About", SettingsMessage::AboutSection))
        .push(content)
        .width(Length::Fill);

    // Connect Device card — only meaningful when this cube is signed
    // in to Connect (device_id populated). For local-daemon installs
    // we keep the About section minimal.
    if let Some(device_id) = &cache.connect_device_id {
        col = col.push(connect_device_card(cache, device_id, state));
    }

    dashboard(menu, cache, col)
}

fn connect_device_card<'a>(
    cache: &'a cache::Cache,
    device_id: &'a str,
    state: &'a AboutSettingsState,
) -> Element<'a, Message> {
    let mut body = Column::new()
        .push(
            Row::new()
                .push(badge::badge(icon::cube_icon()))
                .push(text("Connect Device").bold())
                .padding(10)
                .spacing(20)
                .align_y(Alignment::Center)
                .width(Length::Fill),
        )
        .push(separation().width(Length::Fill))
        .push(Space::new().height(Length::Fixed(10.0)))
        .push(
            Row::new()
                .push(
                    Column::new()
                        .push(text("Device ID").bold())
                        .width(Length::FillPortion(1)),
                )
                .push(
                    text(device_id)
                        .style(coincube_ui::theme::text::secondary)
                        .width(Length::FillPortion(2)),
                )
                .padding([6, 10])
                .spacing(20),
        );
    if let Some(email) = &cache.connect_email {
        body = body.push(
            Row::new()
                .push(
                    Column::new()
                        .push(text("Account").bold())
                        .width(Length::FillPortion(1)),
                )
                .push(
                    text(email.clone())
                        .style(coincube_ui::theme::text::secondary)
                        .width(Length::FillPortion(2)),
                )
                .padding([6, 10])
                .spacing(20),
        );
    }
    body = body.push(
        Row::new()
            .push(
                Column::new()
                    .push(text("Stream").bold())
                    .width(Length::FillPortion(1)),
            )
            .push(
                text(cache.connect_stream_status.tooltip())
                    .style(coincube_ui::theme::text::secondary)
                    .width(Length::FillPortion(2)),
            )
            .padding([6, 10])
            .spacing(20),
    );

    // Re-register row: button + inline status banner.
    let reregister_btn = if state.reregistering {
        button::secondary(None, "Re-registering…")
    } else {
        button::secondary(None, "Re-register this device")
            .on_press(Message::Settings(SettingsMessage::ReregisterConnectDevice))
    };
    let banner: Option<Element<'a, Message>> = match &state.reregister_status {
        Some(Ok(new_id)) => Some(
            text(format!("Re-registered. New device ID: {}", new_id))
                .style(coincube_ui::theme::text::primary)
                .into(),
        ),
        Some(Err(e)) => Some(
            text(format!("Re-registration failed: {}", e))
                .style(coincube_ui::theme::text::primary)
                .into(),
        ),
        None => None,
    };
    let mut footer = Column::new().padding(10).spacing(8).push(reregister_btn);
    if let Some(b) = banner {
        footer = footer.push(b);
    }
    body = body.push(footer);

    card::simple(body).into()
}

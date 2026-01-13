//! View functions for business settings UI.

use iced::widget::{scrollable, Column, Container, Row, Space, Toggler};
use iced::{Alignment, Length};
use liana_ui::{
    component::{badge, button, card, separation, text::*},
    icon, theme,
    widget::Element,
};

use crate::message::{Msg, Section};
use crate::ui::BusinessSettingsUI;

/// Shared layout wrapper for settings views.
pub fn layout<'a, T: Into<Element<'a, Msg>>>(content: T) -> Element<'a, Msg> {
    Container::new(
        scrollable(
            Column::new()
                .push(Space::with_height(30))
                .push(
                    Row::new()
                        .push(Space::with_width(Length::FillPortion(1)))
                        .push(
                            Container::new(content)
                                .width(Length::FillPortion(8))
                                .max_width(1000),
                        )
                        .push(Space::with_width(Length::FillPortion(1))),
                )
                .push(Space::with_height(30)),
        )
        .width(Length::Fill),
    )
    .style(theme::container::background)
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

/// Settings section list view.
pub fn list_view() -> Element<'static, Msg> {
    let header = text("Settings").size(30).bold();

    let general = menu_entry(
        "General",
        icon::wrench_icon(),
        Msg::SelectSection(Section::General),
    );

    let wallet = menu_entry(
        "Wallet",
        icon::wallet_icon(),
        Msg::SelectSection(Section::Wallet),
    );

    let about = menu_entry(
        "About",
        icon::tooltip_icon(),
        Msg::SelectSection(Section::About),
    );

    Column::new()
        .spacing(20)
        .width(Length::Fill)
        .push(header)
        .push(general)
        .push(wallet)
        .push(about)
        .into()
}

/// General settings section view.
pub fn general_view(state: &BusinessSettingsUI) -> Element<'_, Msg> {
    let header = section_header("General");

    let fiat_card = card::simple(
        Column::new().spacing(20).push(
            Row::new()
                .spacing(10)
                .align_y(Alignment::Center)
                .push(text("Fiat price:").bold())
                .push(Space::with_width(Length::Fill))
                .push(
                    Toggler::new(state.fiat_enabled)
                        .on_toggle(Msg::EnableFiat)
                        .style(theme::toggler::primary),
                ),
        ),
    )
    .width(Length::Fill);

    Column::new()
        .spacing(20)
        .push(header)
        .push(fiat_card)
        .width(Length::Fill)
        .into()
}

/// Wallet settings section view.
pub fn wallet_view(state: &BusinessSettingsUI) -> Element<'_, Msg> {
    let header = section_header("Wallet");

    let descriptor = state.wallet.main_descriptor.to_string();
    let descriptor_card = card::simple(
        Column::new()
            .push(text("Wallet descriptor:").bold())
            .push(
                scrollable(
                    Column::new()
                        .push(text(&descriptor).small())
                        .push(Space::with_height(Length::Fixed(5.0))),
                )
                .direction(scrollable::Direction::Horizontal(
                    scrollable::Scrollbar::new().width(5).scroller_width(5),
                )),
            )
            .push(
                Row::new()
                    .spacing(10)
                    .push(Space::with_width(Length::Fill))
                    .push(
                        button::secondary(Some(icon::chip_icon()), "Register on device")
                            .on_press(Msg::RegisterWallet),
                    ),
            )
            .spacing(10),
    )
    .width(Length::Fill);

    Column::new()
        .spacing(20)
        .push(header)
        .push(descriptor_card)
        .width(Length::Fill)
        .into()
}

/// About section view.
pub fn about_view() -> Element<'static, Msg> {
    let header = section_header("About");

    let version_card = card::simple(
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
            .push(Space::with_height(Length::Fixed(10.0)))
            .push(
                Row::new()
                    .push(Space::with_width(Length::Fill))
                    .push(text(format!("liana-gui v{}", liana_gui::VERSION))),
            ),
    );

    Column::new()
        .spacing(20)
        .push(header)
        .push(version_card)
        .width(Length::Fill)
        .into()
}

// Helper functions

fn menu_entry(
    title: &str,
    entry_icon: liana_ui::widget::Text<'static>,
    msg: Msg,
) -> Element<'static, Msg> {
    Container::new(
        iced::widget::Button::new(
            Row::new()
                .push(badge::badge(entry_icon))
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
    .into()
}

fn section_header(title: &'static str) -> Element<'static, Msg> {
    Row::new()
        .spacing(10)
        .align_y(Alignment::Center)
        .push(
            iced::widget::Button::new(text("Settings").size(30).bold())
                .style(theme::button::transparent)
                .on_press(Msg::SelectSection(Section::General)),
        )
        .push(icon::chevron_right().size(30))
        .push(text(title).size(30).bold())
        .into()
}

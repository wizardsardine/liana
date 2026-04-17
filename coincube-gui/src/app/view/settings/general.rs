use iced::widget::{pick_list, Column, Row, Space, Toggler};
use iced::{Alignment, Length};

use coincube_ui::component::card;
use coincube_ui::component::text::*;
use coincube_ui::theme;
use coincube_ui::widget::*;

use crate::app::cache;
use crate::app::menu::Menu;
use crate::app::settings::fiat::PriceSetting;
use crate::app::settings::unit::{BitcoinDisplayUnit, UnitSetting};
use crate::app::view::dashboard;
use crate::app::view::message::*;
use crate::services::fiat::{Currency, ALL_PRICE_SOURCES};

pub fn general_section<'a>(
    menu: &'a Menu,
    cache: &'a cache::Cache,
    new_price_setting: &'a PriceSetting,
    new_unit_setting: &'a UnitSetting,
    currencies_list: &'a [Currency],
    developer_mode: bool,
    show_direction_badges: bool,
) -> Element<'a, Message> {
    let mut col = Column::new()
        .spacing(20)
        .push(super::header("General", SettingsMessage::GeneralSection))
        .push(network_row(cache.network))
        .push(bitcoin_display_unit(new_unit_setting))
        .push(direction_badges_toggle(show_direction_badges))
        .push(fiat_price(new_price_setting, currencies_list));

    if developer_mode {
        col = col.push(toast_testing());
    }

    dashboard(menu, cache, col)
}

fn network_row<'a>(network: coincube_core::miniscript::bitcoin::Network) -> Element<'a, Message> {
    use coincube_core::miniscript::bitcoin::Network;
    let label = match network {
        Network::Bitcoin => "Mainnet",
        Network::Regtest => "Regtest",
        Network::Testnet => "Testnet",
        Network::Signet => "Signet",
        _ => "Unknown",
    };
    card::simple(
        Row::new()
            .spacing(20)
            .align_y(Alignment::Center)
            .push(text("Network:").bold())
            .push(Space::new().width(Length::Fill))
            .push(text(label)),
    )
    .width(Length::Fill)
    .into()
}

fn direction_badges_toggle<'a>(show: bool) -> Element<'a, Message> {
    card::simple(
        Row::new()
            .spacing(20)
            .align_y(Alignment::Center)
            .push(text("Show direction badges on transactions:").bold())
            .push(Space::new().width(Length::Fill))
            .push(
                Toggler::new(show)
                    .on_toggle(|new_val| SettingsMessage::ToggleDirectionBadges(new_val).into())
                    .width(50)
                    .style(theme::toggler::orange),
            ),
    )
    .width(Length::Fill)
    .into()
}

fn toast_testing<'a>() -> Element<'a, Message> {
    let btn = |label: &'static str, level: log::Level| {
        iced::widget::Button::new(text(label).bold())
            .padding([8, 16])
            .style(theme::button::secondary)
            .on_press(SettingsMessage::TestToast(level).into())
    };

    card::simple(
        Column::new()
            .spacing(15)
            .push(text("Toast Testing").bold())
            .push(
                Row::new()
                    .spacing(10)
                    .push(btn("Error", log::Level::Error))
                    .push(btn("Warn", log::Level::Warn))
                    .push(btn("Info", log::Level::Info))
                    .push(btn("Debug", log::Level::Debug))
                    .push(btn("Trace", log::Level::Trace)),
            ),
    )
    .width(Length::Fill)
    .into()
}

pub fn bitcoin_display_unit<'a>(new_unit_setting: &'a UnitSetting) -> Element<'a, Message> {
    card::simple(
        Row::new()
            .spacing(20)
            .align_y(Alignment::Center)
            .push(text("Bitcoin display unit:").bold())
            .push(Space::new().width(Length::Fill))
            .push(text("BTC"))
            .push(
                Toggler::new(matches!(
                    new_unit_setting.display_unit,
                    BitcoinDisplayUnit::Sats
                ))
                .on_toggle(|is_sats| {
                    SettingsMessage::DisplayUnitChanged(if is_sats {
                        BitcoinDisplayUnit::Sats
                    } else {
                        BitcoinDisplayUnit::BTC
                    })
                    .into()
                })
                .width(50)
                .style(theme::toggler::orange),
            )
            .push(text("Sats")),
    )
    .width(Length::Fill)
    .into()
}

pub fn fiat_price<'a>(
    new_price_setting: &'a PriceSetting,
    currencies_list: &'a [Currency],
) -> Element<'a, Message> {
    card::simple(
        Column::new()
            .spacing(20)
            .push(
                Row::new()
                    .spacing(10)
                    .align_y(Alignment::Center)
                    .push(text("Fiat price:").bold())
                    .push(Space::new().width(Length::Fill))
                    .push(
                        Toggler::new(new_price_setting.is_enabled)
                            .on_toggle(|new_selection| FiatMessage::Enable(new_selection).into())
                            .width(50)
                            .style(theme::toggler::orange),
                    ),
            )
            .push_maybe(
                new_price_setting.is_enabled.then_some(
                    Row::new()
                        .spacing(20)
                        .align_y(Alignment::Center)
                        .push(text("Exchange rate source:").bold())
                        .push(Space::new().width(Length::Fill))
                        .push(
                            pick_list(
                                &ALL_PRICE_SOURCES[..],
                                Some(new_price_setting.source),
                                |source| FiatMessage::SourceEdited(source).into(),
                            )
                            .style(theme::pick_list::primary)
                            .padding(10),
                        ),
                ),
            )
            .push_maybe(
                new_price_setting.is_enabled.then_some(
                    Row::new()
                        .spacing(20)
                        .align_y(Alignment::Center)
                        .push(text("Currency:").bold())
                        .push(Space::new().width(Length::Fill))
                        .push(
                            pick_list(
                                currencies_list,
                                Some(new_price_setting.currency),
                                |currency| FiatMessage::CurrencyEdited(currency).into(),
                            )
                            .style(theme::pick_list::primary)
                            .padding(10),
                        ),
                ),
            )
            .push_maybe(
                new_price_setting
                    .source
                    .attribution()
                    .filter(|_| new_price_setting.is_enabled)
                    .map(|s| {
                        Row::new()
                            .spacing(20)
                            .align_y(Alignment::Center)
                            .push(Space::new().width(Length::Fill))
                            .push(text(s))
                    }),
            ),
    )
    .width(Length::Fill)
    .into()
}

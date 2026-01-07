use iced::widget::{pick_list, Column, Row, Space, Toggler};
use iced::{Alignment, Length};

use coincube_ui::component::card;
use coincube_ui::component::text::*;
use coincube_ui::theme;
use coincube_ui::widget::*;

use crate::app::cache;
use crate::app::error::Error;
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
    warning: Option<&Error>,
) -> Element<'a, Message> {
    dashboard(
        menu,
        cache,
        warning,
        Column::new()
            .spacing(20)
            .push(super::header("General", SettingsMessage::GeneralSection))
            .push(bitcoin_display_unit(new_unit_setting))
            .push(fiat_price(new_price_setting, currencies_list)),
    )
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

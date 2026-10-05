use iced::{
    widget::{column, row, tooltip, Space, Toggler},
    Alignment, Length,
};

use super::{header, SETTING_MSG};

use liana_ui::{
    color,
    component::{
        card,
        panels::setting::SectionKind,
        pick_list,
        text::{legacy, Text},
        tooltip_custom,
    },
    icon, theme,
    widget::*,
};

use crate::app::cache;
use crate::app::error::Error;
use crate::app::menu::Menu;
use crate::app::settings::fiat::PriceSetting;
use crate::app::view::dashboard;
use crate::app::view::message::*;
use crate::app::view::settings::SettingsMessage;
use crate::services::fiat::{Currency, ALL_PRICE_SOURCES};
use crate::t;

pub fn general_section<'a>(
    cache: &'a cache::Cache,
    new_price_setting: &'a PriceSetting,
    currencies_list: &'a [Currency],
    warning: Option<&'a Error>,
) -> Element<'a, Message> {
    let header = header(
        Some(SETTING_MSG),
        Some(SectionKind::General.title()),
        Some(SettingsMessage::GeneralSection.into()),
    );

    let fiat_price = fiat_price(new_price_setting, currencies_list);

    let content = column![header, fiat_price].spacing(20);

    dashboard(&Menu::Settings, cache, warning, content)
}

pub fn fiat_price<'a>(
    new_price_setting: &'a PriceSetting,
    currencies_list: &'a [Currency],
) -> Element<'a, Message> {
    let fiat_tooltip = tooltip_custom(
        legacy::text(t!("settings-fiat-price-tooltip")),
        icon::warning_icon().color(color::ORANGE),
        tooltip::Position::Bottom,
    );
    let toggler = Toggler::new(new_price_setting.is_enabled)
        .on_toggle(|new_selection| FiatMessage::Enable(new_selection).into())
        .style(theme::toggler::primary);
    let fiat = row![
        legacy::text(t!("settings-fiat-price")).bold(),
        fiat_tooltip,
        Space::fill_width(),
        toggler
    ]
    .spacing(10)
    .align_y(Alignment::Center);

    let source_picker = pick_list::pick_list(
        &ALL_PRICE_SOURCES[..],
        Some(new_price_setting.source),
        |source| FiatMessage::SourceEdited(source).into(),
    )
    .padding(10);
    let source = new_price_setting.is_enabled.then_some(
        row![
            legacy::text(t!("settings-exchange-rate-source")).bold(),
            Space::fill_width(),
            source_picker
        ]
        .spacing(20)
        .align_y(Alignment::Center),
    );

    let currency_picker = pick_list::pick_list(
        currencies_list,
        Some(new_price_setting.currency),
        |currency| FiatMessage::CurrencyEdited(currency).into(),
    )
    .padding(10);
    let currency = new_price_setting.is_enabled.then_some(
        row![
            legacy::text(t!("settings-currency")).bold(),
            Space::fill_width(),
            currency_picker
        ]
        .spacing(20)
        .align_y(Alignment::Center),
    );

    let attribution = new_price_setting
        .source
        .attribution()
        .filter(|_| new_price_setting.is_enabled)
        .map(|s| row![Space::fill_width(), legacy::text(s)].align_y(Alignment::Center));

    let content = column![fiat, source, currency, attribution].spacing(20);

    card::simple(content).width(Length::Fill).into()
}

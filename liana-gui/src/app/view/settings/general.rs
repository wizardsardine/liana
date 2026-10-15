use iced::{
    widget::{column, row, tooltip, Space},
    Alignment, Length,
};

use super::{header, SETTING_MSG};

use liana_ui::{
    component::{
        card,
        checkbox::{toggler_button, TogglerSize},
        panels::setting::{setting_row, SectionKind},
        pick_list,
        text::new,
        tooltip::help_text,
        tooltip_custom,
    },
    icon,
    spacing::VSpacing,
    theme,
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
use liana_i18n::{self as i18n, SupportedLocale};

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

    let language = language();
    let fiat_price = fiat_price(new_price_setting, currencies_list);

    let content = column![header, language, fiat_price].spacing(VSpacing::L);

    dashboard(&Menu::Settings, cache, warning, content)
}

pub fn language<'a>() -> Element<'a, Message> {
    let picker = pick_list::pick_list(
        &SupportedLocale::ALL[..],
        Some(i18n::current_locale()),
        |locale| SettingsMessage::LanguageEdited(locale).into(),
    )
    .padding(pick_list::PICK_LIST_PADDING);
    let language = setting_row(t!("settings-language"), None, picker);
    let description =
        new::caption(t!("settings-language-description")).style(theme::text::secondary);

    card::simple(column![language, description].spacing(VSpacing::S))
        .width(Length::Fill)
        .into()
}

pub fn fiat_price<'a>(
    new_price_setting: &'a PriceSetting,
    currencies_list: &'a [Currency],
) -> Element<'a, Message> {
    let fiat_tooltip = tooltip_custom(
        help_text(t!("settings-fiat-price-tooltip")),
        icon::warning_icon().style(theme::text::warning),
        tooltip::Position::Bottom,
    );
    let toggler = toggler_button(
        new_price_setting.is_enabled,
        TogglerSize::Normal,
        |enabled| FiatMessage::Enable(enabled).into(),
    );
    let fiat = setting_row(
        t!("settings-fiat-price"),
        Some(fiat_tooltip.into()),
        toggler,
    );

    let source_picker = pick_list::pick_list(
        &ALL_PRICE_SOURCES[..],
        Some(new_price_setting.source),
        |source| FiatMessage::SourceEdited(source).into(),
    )
    .padding(pick_list::PICK_LIST_PADDING);
    let source = new_price_setting
        .is_enabled
        .then(|| setting_row(t!("settings-exchange-rate-source"), None, source_picker));

    let currency_picker = pick_list::pick_list(
        currencies_list,
        Some(new_price_setting.currency),
        |currency| FiatMessage::CurrencyEdited(currency).into(),
    )
    .padding(pick_list::PICK_LIST_PADDING);
    let currency = new_price_setting
        .is_enabled
        .then(|| setting_row(t!("settings-currency"), None, currency_picker));

    let attribution = new_price_setting
        .source
        .attribution()
        .filter(|_| new_price_setting.is_enabled)
        .map(|s| {
            row![
                Space::fill_width(),
                new::caption(s).style(theme::text::secondary)
            ]
            .align_y(Alignment::Center)
        });

    let content = column![fiat, source, currency, attribution].spacing(VSpacing::L);

    card::simple(content).width(Length::Fill).into()
}

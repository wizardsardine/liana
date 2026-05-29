use iced::widget::{tooltip, Column, Row, Space, Toggler};
use iced::{Alignment, Length};
use liana_ui::component::setting::SectionKind;

use super::{header, SETTING_MSG};

use liana_ui::color;
use liana_ui::component::card;
use liana_ui::component::pick_list;
use liana_ui::component::text::*;
use liana_ui::component::tooltip_custom;
use liana_ui::icon;
use liana_ui::theme;
use liana_ui::widget::*;

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

    dashboard(
        &Menu::Settings,
        cache,
        warning,
        Column::new()
            .spacing(20)
            .push(header)
            .push(language())
            .push(fiat_price(new_price_setting, currencies_list)),
    )
}

pub fn language<'a>() -> Element<'a, Message> {
    card::simple(
        Column::new()
            .spacing(10)
            .push(
                Row::new()
                    .spacing(20)
                    .align_y(Alignment::Center)
                    .push(text(t!("settings-language")).bold())
                    .push(Space::with_width(Length::Fill))
                    .push(
                        pick_list::pick_list(
                            &SupportedLocale::ALL[..],
                            Some(i18n::current_locale()),
                            |locale| SettingsMessage::LanguageEdited(locale).into(),
                        )
                        .padding(10),
                    ),
            )
            .push(text(t!("settings-language-description")).style(theme::text::secondary)),
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
                    .push(text(t!("settings-fiat-price")).bold())
                    .push(tooltip_custom(
                        t!("settings-fiat-price-tooltip"),
                        icon::warning_icon().color(color::ORANGE),
                        tooltip::Position::Bottom,
                    ))
                    .push(Space::with_width(Length::Fill))
                    .push(
                        Toggler::new(new_price_setting.is_enabled)
                            .on_toggle(|new_selection| FiatMessage::Enable(new_selection).into())
                            .style(theme::toggler::primary),
                    ),
            )
            .push_maybe(
                new_price_setting.is_enabled.then_some(
                    Row::new()
                        .spacing(20)
                        .align_y(Alignment::Center)
                        .push(text(t!("settings-exchange-rate-source")).bold())
                        .push(Space::with_width(Length::Fill))
                        .push(
                            pick_list::pick_list(
                                &ALL_PRICE_SOURCES[..],
                                Some(new_price_setting.source),
                                |source| FiatMessage::SourceEdited(source).into(),
                            )
                            .padding(10),
                        ),
                ),
            )
            .push_maybe(
                new_price_setting.is_enabled.then_some(
                    Row::new()
                        .spacing(20)
                        .align_y(Alignment::Center)
                        .push(text(t!("settings-currency")).bold())
                        .push(Space::with_width(Length::Fill))
                        .push(
                            pick_list::pick_list(
                                currencies_list,
                                Some(new_price_setting.currency),
                                |currency| FiatMessage::CurrencyEdited(currency).into(),
                            )
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
                            .push(Space::with_width(Length::Fill))
                            .push(text(s))
                    }),
            ),
    )
    .width(Length::Fill)
    .into()
}

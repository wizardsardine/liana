//! View functions for business settings UI.

use iced::{
    widget::{column, row, Space, Toggler},
    Alignment, Length,
};
use liana_i18n::t;
use liana_ui::{
    component::{
        self, badge,
        button::btn_register_on_device,
        card,
        panels::setting::{header, settings_section, SectionKind},
        pick_list, scrollable, separation,
        text::{legacy, Text},
    },
    theme,
    widget::{Element, SpaceExt},
};

use crate::{
    settings::{
        message::{Msg, Section},
        ui::BusinessSettingsUI,
    },
    VERSION,
};

const SETTING_MSG: Msg = Msg::Home;

/// Settings section list view.
pub fn list_view() -> Element<'static, Msg> {
    let wallet = settings_section(SectionKind::Wallet, Msg::SelectSection(Section::Wallet));
    let general = settings_section(SectionKind::General, Msg::SelectSection(Section::General));
    let about = settings_section(SectionKind::About, Msg::SelectSection(Section::About));

    component::panels::setting::section_list(vec![general, wallet, about])
}

/// Wallet settings section view.
pub fn wallet_view(state: &BusinessSettingsUI) -> Element<'_, Msg> {
    let header = header(Some(SETTING_MSG), Some(SectionKind::Wallet.title()), None);

    let descriptor = state.wallet.main_descriptor.to_string();
    let title = legacy::text(t!("settings-wallet-descriptor")).bold();
    let descriptor_s = scrollable::horizontal_thin(legacy::text(&descriptor).small());
    let btn_row = row![
        Space::fill_width(),
        btn_register_on_device(Msg::RegisterWallet)
    ];
    let descriptor_card =
        card::simple(column![title, descriptor_s, btn_row].spacing(10)).width(Length::Fill);

    column![header, descriptor_card]
        .spacing(20)
        .width(Length::Fill)
        .into()
}

/// General settings section view with fiat price configuration.
pub fn general_view(
    fiat_enabled: bool,
    currency: crate::settings::BackendCurrency,
) -> Element<'static, Msg> {
    let header = header(Some(SETTING_MSG), Some(SectionKind::General.title()), None);

    let toggler = Toggler::new(fiat_enabled)
        .on_toggle(Msg::FiatEnable)
        .style(theme::toggler::primary);
    let fiat = row![
        legacy::text(t!("settings-fiat-price")).bold(),
        Space::fill_width(),
        toggler
    ]
    .spacing(10)
    .align_y(Alignment::Center);

    let currency_picker = pick_list::pick_list(
        crate::settings::ALL_BACKEND_CURRENCIES,
        Some(currency),
        Msg::FiatCurrencyEdited,
    )
    .padding(10);
    let currency = fiat_enabled.then_some(
        row![
            legacy::text(t!("settings-currency")).bold(),
            Space::fill_width(),
            currency_picker
        ]
        .spacing(20)
        .align_y(Alignment::Center),
    );

    let fiat_card = card::simple(column![fiat, currency].spacing(20)).width(Length::Fill);

    column![header, fiat_card]
        .spacing(20)
        .width(Length::Fill)
        .into()
}

/// About section view.
pub fn about_view() -> Element<'static, Msg> {
    let header = header(Some(SETTING_MSG), Some(SectionKind::About.title()), None);

    let version_title = row![
        badge::tooltip(),
        legacy::text(t!("settings-version")).bold()
    ]
    .padding(10)
    .spacing(20)
    .align_y(Alignment::Center)
    .width(Length::Fill);
    let version = row![
        Space::fill_width(),
        legacy::text(format!("liana-business v{VERSION}"))
    ];
    let version_card = card::simple(column![
        version_title,
        separation().width(Length::Fill),
        Space::with_height(10),
        version
    ]);

    column![header, version_card]
        .spacing(20)
        .width(Length::Fill)
        .into()
}

//! View functions for business settings UI.

use iced::{
    widget::{column, row, Space},
    Length,
};
use liana_i18n::t;
use liana_ui::{
    component::{
        self,
        button::btn_register_on_device,
        card,
        checkbox::{toggler_button, TogglerSize},
        panels::setting::{header, setting_row, settings_section, version_card, SectionKind},
        pick_list, scrollable,
        text::new,
    },
    spacing::VSpacing,
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
    let title = new::h3_semi(t!("settings-wallet-descriptor"));
    let descriptor_s = scrollable::horizontal_thin(new::caption(&descriptor));
    let btn_row = row![
        Space::fill_width(),
        btn_register_on_device(Msg::RegisterWallet)
    ];
    let descriptor_card = card::simple(column![title, descriptor_s, btn_row].spacing(VSpacing::S))
        .width(Length::Fill);

    column![header, descriptor_card]
        .spacing(VSpacing::L)
        .width(Length::Fill)
        .into()
}

/// General settings section view with fiat price configuration.
pub fn general_view(
    fiat_enabled: bool,
    currency: crate::settings::BackendCurrency,
) -> Element<'static, Msg> {
    let header = header(Some(SETTING_MSG), Some(SectionKind::General.title()), None);

    let toggler = toggler_button(fiat_enabled, TogglerSize::Normal, Msg::FiatEnable);
    let fiat = setting_row(t!("settings-fiat-price"), None, toggler);

    let currency_picker = pick_list::pick_list(
        crate::settings::ALL_BACKEND_CURRENCIES,
        Some(currency),
        Msg::FiatCurrencyEdited,
    )
    .padding(pick_list::PICK_LIST_PADDING);
    let currency =
        fiat_enabled.then(|| setting_row(t!("settings-currency"), None, currency_picker));

    let fiat_card = card::simple(column![fiat, currency].spacing(VSpacing::L)).width(Length::Fill);

    column![header, fiat_card]
        .spacing(VSpacing::L)
        .width(Length::Fill)
        .into()
}

/// About section view.
pub fn about_view() -> Element<'static, Msg> {
    let header = header(Some(SETTING_MSG), Some(SectionKind::About.title()), None);

    let version_card = version_card(vec![format!("liana-business v{VERSION}")]);

    column![header, version_card]
        .spacing(VSpacing::L)
        .width(Length::Fill)
        .into()
}

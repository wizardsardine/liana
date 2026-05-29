//! View functions for business settings UI.

use iced::{
    widget::{Column, Row, Space, Toggler},
    Alignment, Length,
};
use liana_i18n::{self as i18n, t, SupportedLocale};
use liana_ui::{
    component::{
        self, badge, button, card, pick_list, scrollable, separation,
        setting::{header, settings_section, SectionKind},
        text::*,
    },
    icon, theme,
    widget::{ColumnExt, Element, SpaceExt},
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

    component::setting::section_list(vec![general, wallet, about])
}

/// Wallet settings section view.
pub fn wallet_view(state: &BusinessSettingsUI) -> Element<'_, Msg> {
    let header = header(Some(SETTING_MSG), Some(SectionKind::Wallet.title()), None);

    let descriptor = state.wallet.main_descriptor.to_string();
    let descriptor_card = card::simple(
        Column::new()
            .push(text(t!("settings-wallet-descriptor")).bold())
            .push(scrollable::horizontal_thin(
                Column::new().push(text(&descriptor).small()),
            ))
            .push(
                Row::new()
                    .spacing(10)
                    .push(Space::with_width(Length::Fill))
                    .push(
                        button::secondary(
                            Some(icon::chip_icon()),
                            t!("settings-register-on-device"),
                        )
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

/// General settings section view with fiat price configuration.
pub fn general_view(
    fiat_enabled: bool,
    currency: crate::settings::BackendCurrency,
) -> Element<'static, Msg> {
    let header = header(Some(SETTING_MSG), Some(SectionKind::General.title()), None);

    let fiat_card = card::simple(
        Column::new()
            .spacing(20)
            .push(
                Row::new()
                    .spacing(10)
                    .align_y(Alignment::Center)
                    .push(text(t!("settings-fiat-price")).bold())
                    .push(Space::with_width(Length::Fill))
                    .push(
                        Toggler::new(fiat_enabled)
                            .on_toggle(Msg::FiatEnable)
                            .style(theme::toggler::primary),
                    ),
            )
            .push_maybe(
                fiat_enabled.then_some(
                    Row::new()
                        .spacing(20)
                        .align_y(Alignment::Center)
                        .push(text(t!("settings-currency")).bold())
                        .push(Space::with_width(Length::Fill))
                        .push(
                            pick_list::pick_list(
                                crate::settings::ALL_BACKEND_CURRENCIES,
                                Some(currency),
                                Msg::FiatCurrencyEdited,
                            )
                            .padding(10),
                        ),
                ),
            ),
    )
    .width(Length::Fill);

    let language_card = card::simple(
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
                            Msg::LanguageEdited,
                        )
                        .padding(10),
                    ),
            )
            .push(text(t!("settings-language-description")).style(theme::text::secondary)),
    )
    .width(Length::Fill);

    Column::new()
        .spacing(20)
        .push(header)
        .push(language_card)
        .push(fiat_card)
        .width(Length::Fill)
        .into()
}

/// About section view.
pub fn about_view() -> Element<'static, Msg> {
    let header = header(Some(SETTING_MSG), Some(SectionKind::About.title()), None);

    let version_card = card::simple(
        Column::new()
            .push(
                Row::new()
                    .push(badge::tooltip())
                    .push(text(t!("settings-version")).bold())
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
                    .push(text(format!("liana-business v{VERSION}"))),
            ),
    );

    Column::new()
        .spacing(20)
        .push(header)
        .push(version_card)
        .width(Length::Fill)
        .into()
}

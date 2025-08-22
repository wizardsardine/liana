#[cfg(feature = "dev-meld")]
use iced::{
    widget::{text, Space},
    Alignment, Length,
};

#[cfg(feature = "dev-meld")]
use liana::miniscript::bitcoin::Network;

#[cfg(feature = "dev-meld")]
use liana_ui::{
    color,
    component::{button as ui_button, form},
    icon::{bitcoin_icon, previous_icon},
    theme,
    widget::*,
};

#[cfg(feature = "dev-meld")]
use crate::app::{
    view::{MeldBuySellMessage, Message as ViewMessage},
};



#[cfg(feature = "dev-meld")]
#[derive(Debug, Clone)]
pub struct MeldBuySellPanel {
    pub wallet_address: form::Value<String>,
    pub country_code: form::Value<String>,
    pub source_amount: form::Value<String>,
    pub loading: bool,
    pub error: Option<String>,
    pub network: Network,
    pub widget_url: Option<String>,
    pub widget_session_created: Option<String>,
}

#[cfg(feature = "dev-meld")]
impl MeldBuySellPanel {
    pub fn new(network: Network) -> Self {
        Self {
            wallet_address: form::Value {
                value: "2N3oefVeg6stiTb5Kh3ozCSkaqmx91FDbsm".to_string(),
                warning: None,
                valid: true,
            },
            country_code: form::Value {
                value: "US".to_string(),
                warning: None,
                valid: true,
            },
            source_amount: form::Value {
                value: "60".to_string(),
                warning: None,
                valid: true,
            },

            loading: false,
            error: None,
            network,
            widget_url: None,
            widget_session_created: None,
        }
    }

    pub fn start_session(&mut self) {
        self.loading = true;
        self.error = None;
    }

    pub fn set_error(&mut self, error: String) {
        self.error = Some(error);
        self.loading = false;
    }

    pub fn session_created(&mut self, widget_url: String) {
        self.loading = false;
        self.error = None;
        self.widget_url = Some(widget_url.clone());
        self.widget_session_created = Some(widget_url);
    }

    pub fn set_wallet_address(&mut self, address: String) {
        self.wallet_address.value = address;
        self.wallet_address.valid = !self.wallet_address.value.is_empty() && self.validate_wallet_address();
        self.update_wallet_address_warning();
    }

    fn validate_wallet_address(&self) -> bool {
        if self.wallet_address.value.is_empty() {
            return false;
        }

        // Network-aware Bitcoin address validation
        let addr = &self.wallet_address.value;
        let is_valid_length = addr.len() >= 26 && addr.len() <= 62;

        if !is_valid_length {
            return false;
        }

        match self.network {
            Network::Bitcoin => {
                // Mainnet addresses: 1, 3, bc1
                addr.starts_with('1') || addr.starts_with('3') || addr.starts_with("bc1")
            }
            Network::Testnet | Network::Signet | Network::Regtest => {
                // Testnet addresses: 2, tb1, bcrt1
                addr.starts_with('2') || addr.starts_with("tb1") || addr.starts_with("bcrt1")
            }
            _ => false, // Unknown network
        }
    }

    fn update_wallet_address_warning(&mut self) {
        if self.wallet_address.value.is_empty() {
            self.wallet_address.warning = None;
            return;
        }

        // Simplified validation - Meld API handles network-specific requirements
        self.wallet_address.warning = if self.validate_wallet_address() {
            None
        } else {
            Some("Invalid Bitcoin address format")
        };
    }

    pub fn set_country_code(&mut self, code: String) {
        self.country_code.value = code;
        self.country_code.valid = !self.country_code.value.is_empty();
    }

    pub fn set_source_amount(&mut self, amount: String) {
        self.source_amount.value = amount;
        self.source_amount.valid = !self.source_amount.value.is_empty() && self.source_amount.value.parse::<f64>().is_ok();
    }



    pub fn is_form_valid(&self) -> bool {
        self.wallet_address.valid
            && self.country_code.valid
            && self.source_amount.valid
            && !self.wallet_address.value.is_empty()
            && !self.country_code.value.is_empty()
            && !self.source_amount.value.is_empty()
    }
}

#[cfg(feature = "dev-meld")]
pub fn meld_buysell_view(state: &MeldBuySellPanel) -> Element<'_, ViewMessage> {
    meld_buysell_view_with_webview(state, None)
}

#[cfg(feature = "dev-meld")]
pub fn meld_buysell_view_with_webview<'a>(
    state: &'a MeldBuySellPanel,
    webview_widget: Option<Element<'a, ViewMessage>>
) -> Element<'a, ViewMessage> {
    Container::new({
        let mut column = Column::new();

        // Check if webview widget is provided (before consuming it)
        let has_webview = webview_widget.is_some();

        // Only show Previous button if we have a webview session active
        if has_webview {
            column = column.push(
                Row::new()
                    .push(
                        Button::new(
                            Row::new()
                                .push(previous_icon().color(color::GREY_2))
                                .push(Space::with_width(Length::Fixed(5.0)))
                                .push(text("Previous").color(color::GREY_2))
                                .spacing(5)
                                .align_y(Alignment::Center)
                        )
                        .style(|_theme, _status| iced::widget::button::Style {
                            background: None,
                            text_color: color::GREY_2,
                            border: iced::Border::default(),
                            shadow: iced::Shadow::default(),
                        })
                        .on_press(ViewMessage::MeldBuySell(MeldBuySellMessage::GoBackToForm))
                    )
                    .push(Space::with_width(Length::Fill))
                    .align_y(Alignment::Center)
            );
        }

        // Insert webview widget right after the Previous button if provided
        if let Some(webview) = webview_widget {
            column = column
                .push(Space::with_height(Length::Fixed(20.0)))
                .push(webview);
        }

        // Only show form content if no session has been created (i.e., no webview is active)
        column
            .push_maybe(if state.widget_url.is_none() && !has_webview {
                Some(
                    Column::new()
                        .push(Space::with_height(Length::Fixed(10.0)))
                        .push(
                            Row::new()
                            .push(Space::with_width(Length::Fill))
                            .push(
                                Row::new()
                                    .push(
                                        Container::new(bitcoin_icon().size(24))
                                            .style(theme::container::border)
                                            .padding(10)
                                    )
                                    .push(Space::with_width(Length::Fixed(15.0)))
                                    .push(
                                        Column::new()
                                            .push(text("COINCUBE").size(16).color(color::ORANGE))
                                            .push(text("BUY/SELL").size(14).color(color::GREY_3))
                                            .spacing(2)
                                    )
                                    .align_y(Alignment::Center)
                            )
                            .push(Space::with_width(Length::Fill))
                            .align_y(Alignment::Center)
                        )
                )
            } else {
                None
            })
            .push_maybe(if state.widget_url.is_none() && !has_webview {
                Some(Space::with_height(Length::Fixed(10.0)))
            } else {
                None
            })
            .push_maybe(if state.widget_url.is_none() && !has_webview {
                Some(meld_form_content(state))
            } else {
                None
            })
            .align_x(Alignment::Center)
            .spacing(5) // Reduced spacing for more compact layout
            .max_width(600)
            .width(Length::Fill)
    })
    .padding(iced::Padding::new(2.0).left(40.0).right(40.0).bottom(20.0)) // further reduced padding for compact layout
    .center_x(Length::Fill)
    .into()
}

#[cfg(feature = "dev-meld")]
fn meld_form_content(state: &MeldBuySellPanel) -> Element<'_, ViewMessage> {
    Column::new()
        .push_maybe(state.error.as_ref().map(|err| {
            Container::new(
                text(err)
                    .size(14)
                    .color(color::RED)
            )
            .padding(10)
            .style(theme::card::invalid)
        }))
        .push(Space::with_height(Length::Fixed(20.0)))
        .push(
            Column::new()
                .push(text("Wallet Address").size(14).color(color::GREY_3))
                .push(Space::with_height(Length::Fixed(5.0)))
                .push({
                    let placeholder = match state.network {
                        Network::Bitcoin => "Enter mainnet Bitcoin address (1, 3, bc1)",
                        _ => "Enter testnet Bitcoin address (2, tb1, bcrt1)",
                    };

                    form::Form::new_trimmed(placeholder, &state.wallet_address, |value| {
                        ViewMessage::MeldBuySell(MeldBuySellMessage::WalletAddressChanged(value))
                    })
                    .size(16)
                    .padding(15)
                })
                .spacing(5)
        )
        .push(Space::with_height(Length::Fixed(20.0)))
        .push(
            Row::new()
                .push(
                    Column::new()
                        .push(text("Country Code").size(14).color(color::GREY_3))
                        .push(Space::with_height(Length::Fixed(5.0)))
                        .push(
                            form::Form::new_trimmed("US", &state.country_code, |value| {
                                ViewMessage::MeldBuySell(MeldBuySellMessage::CountryCodeChanged(value))
                            })
                            .size(16)
                            .padding(15)
                        )
                        .spacing(5)
                        .width(Length::FillPortion(1))
                )
                .push(Space::with_width(Length::Fixed(20.0)))
                .push(
                    Column::new()
                        .push(text("Amount (USD)").size(14).color(color::GREY_3))
                        .push(Space::with_height(Length::Fixed(5.0)))
                        .push(
                            form::Form::new_trimmed("60", &state.source_amount, |value| {
                                ViewMessage::MeldBuySell(MeldBuySellMessage::SourceAmountChanged(value))
                            })
                            .size(16)
                            .padding(15)
                        )
                        .spacing(5)
                        .width(Length::FillPortion(1))
                )
        )

        .push(Space::with_height(Length::Fixed(30.0)))
        .push(
            if state.loading {
                ui_button::secondary(None, "Creating Session...")
                    .width(Length::Fill)
            } else if state.is_form_valid() {
                ui_button::primary(None, "Create Widget Session")
                    .on_press(ViewMessage::MeldBuySell(MeldBuySellMessage::CreateSession))
                    .width(Length::Fill)
            } else {
                ui_button::secondary(None, "Create Widget Session")
                    .width(Length::Fill)
            }
        )
        .push_maybe(network_info_panel(state))
        .align_x(Alignment::Center)
        .spacing(5)
        .max_width(500)
        .width(Length::Fill)
        .into()
}

#[cfg(feature = "dev-meld")]
fn network_info_panel(state: &MeldBuySellPanel) -> Option<Element<'_, ViewMessage>> {
    // Simplified network info - Meld API handles provider-specific requirements
    match state.network {
        Network::Bitcoin => Some(
            Container::new(
                Column::new()
                    .push(text("🌐 Mainnet Network").size(14).color(color::ORANGE))
                    .push(Space::with_height(Length::Fixed(5.0)))
                    .push(text("Using Bitcoin mainnet with Transak payment provider").size(12).color(color::GREY_3))
                    .push(text("Address formats: 1..., 3..., bc1...").size(12).color(color::GREY_3))
                    .spacing(2)
            )
            .padding(10)
            .style(theme::card::simple)
            .into()
        ),
        _ => Some(
            Container::new(
                Column::new()
                    .push(text("🧪 Testnet Network").size(14).color(color::ORANGE))
                    .push(Space::with_height(Length::Fixed(5.0)))
                    .push(text("Using Bitcoin testnet with Transak payment provider").size(12).color(color::GREY_3))
                    .push(text("Address formats: 2..., tb1..., bcrt1...").size(12).color(color::GREY_3))
                    .spacing(2)
            )
            .padding(10)
            .style(theme::card::simple)
            .into()
        )
    }
}
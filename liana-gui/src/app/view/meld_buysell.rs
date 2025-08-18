#[cfg(feature = "dev-meld")]
use iced::{
    widget::{text, Space, pick_list},
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
    buysell::ServiceProvider,
    view::{MeldBuySellMessage, Message as ViewMessage},
};

#[cfg(all(feature = "dev-meld", feature = "webview"))]
use iced_webview;

#[cfg(feature = "dev-meld")]
#[derive(Debug, Clone)]
pub struct MeldBuySellPanel {
    pub wallet_address: form::Value<String>,
    pub country_code: form::Value<String>,
    pub source_amount: form::Value<String>,
    pub selected_provider: ServiceProvider,
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
            selected_provider: ServiceProvider::Transak, // Only Transak for now
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

        self.wallet_address.warning = match self.selected_provider {
            ServiceProvider::Transak => {
                // Transak testing requires testnet addresses
                if self.network == Network::Bitcoin {
                    Some("Transak testing requires testnet. Switch to testnet network or use testnet address")
                } else if self.validate_wallet_address() {
                    None
                } else {
                    Some("For Transak testing, use testnet address like: 2N3oefVeg6stiTb5Kh3ozCSkaqmx91FDbsm")
                }
            }
            _ => {
                if self.validate_wallet_address() {
                    None
                } else {
                    match self.network {
                        Network::Bitcoin => Some("Please enter a valid mainnet Bitcoin address (1, 3, or bc1)"),
                        _ => Some("Please enter a valid testnet Bitcoin address (2, tb1, or bcrt1)"),
                    }
                }
            }
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

    pub fn set_selected_provider(&mut self, provider: ServiceProvider) {
        self.selected_provider = provider;
        // Re-validate wallet address when provider changes
        self.wallet_address.valid = !self.wallet_address.value.is_empty() && self.validate_wallet_address();
        self.update_wallet_address_warning();
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
    Container::new(
        Column::new()
            .push(
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
            )
            .push_maybe(if state.widget_session_created.is_none() {
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
            .push(Space::with_height(Length::Fixed(10.0)))
            .push(meld_form_content(state))
            .align_x(Alignment::Center)
            .spacing(10)
            .max_width(600)
            .width(Length::Fill)
    )
    .padding(iced::Padding::new(5.0).left(40.0).right(40.0).bottom(40.0)) // further reduced top padding
    .center_x(Length::Fill)
    .into()
}

#[cfg(feature = "dev-meld")]
fn meld_form_content(state: &MeldBuySellPanel) -> Element<'_, ViewMessage> {
    let providers = vec![ServiceProvider::Transak]; // Only Transak for now

    // If we have a widget URL, show success state
    if let Some(widget_url) = &state.widget_url {
        tracing::info!("Rendering meld form content using url: {}", widget_url);
        return success_content(widget_url);
    }

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
                    let placeholder = match (state.selected_provider, state.network) {
                        (ServiceProvider::Transak, _) => "Testnet address for Transak testing",
                        (_, Network::Bitcoin) => "Enter mainnet Bitcoin address (1, 3, bc1)",
                        (_, _) => "Enter testnet Bitcoin address (2, tb1, bcrt1)",
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
        .push(Space::with_height(Length::Fixed(20.0)))
        .push(
            Column::new()
                .push(text("Service Provider").size(14).color(color::GREY_3))
                .push(Space::with_height(Length::Fixed(5.0)))
                .push(
                    pick_list(
                        providers,
                        Some(state.selected_provider),
                        |provider| ViewMessage::MeldBuySell(MeldBuySellMessage::ProviderSelected(provider))
                    )
                    .padding(15)
                    .width(Length::Fill)
                )
                .spacing(5)
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
    match (state.selected_provider, state.network) {
        (ServiceProvider::Transak, Network::Bitcoin) => Some(
            Container::new(
                Column::new()
                    .push(text("⚠️ Network Mismatch").size(14).color(color::RED))
                    .push(Space::with_height(Length::Fixed(5.0)))
                    .push(text("Transak testing requires testnet network").size(12).color(color::GREY_3))
                    .push(text("Switch to testnet with: cargo run --bin liana-gui --features dev-meld -- --testnet").size(12).color(color::GREY_3))
                    .spacing(2)
            )
            .padding(10)
            .style(theme::card::invalid)
            .into()
        ),
        (ServiceProvider::Transak, _) => Some(
            Container::new(
                Column::new()
                    .push(text("💡 Transak Testing Info").size(14).color(color::ORANGE))
                    .push(Space::with_height(Length::Fixed(5.0)))
                    .push(text("Using testnet network with default Transak test address").size(12).color(color::GREY_3))
                    .push(text("Default address: 2N3oefVeg6stiTb5Kh3ozCSkaqmx91FDbsm").size(12).color(color::GREY_3))
                    .spacing(2)
            )
            .padding(10)
            .style(theme::card::simple)
            .into()
        ),
        (_, Network::Bitcoin) => Some(
            Container::new(
                Column::new()
                    .push(text("🌐 Mainnet Network").size(14).color(color::ORANGE))
                    .push(Space::with_height(Length::Fixed(5.0)))
                    .push(text("Using Bitcoin mainnet - use real addresses").size(12).color(color::GREY_3))
                    .push(text("Address formats: 1..., 3..., bc1...").size(12).color(color::GREY_3))
                    .spacing(2)
            )
            .padding(10)
            .style(theme::card::simple)
            .into()
        ),
        _ => None
    }
}

#[cfg(feature = "dev-meld")]
fn success_content(widget_url: &str) -> Element<'_, ViewMessage> {
    Column::new()
        .push({
            // Show the webview widget with a launch button
            #[cfg(feature = "webview")]
            {
                crate::app::view::webview::meld_webview_widget(widget_url, None, false)
            }
            #[cfg(not(feature = "webview"))]
            {
                // Fallback UI when webview is not available
                Container::new(
                    Column::new()
                        .push(text("🌐 Meld Widget").size(16).color(color::GREEN))
                        .push(Space::with_height(Length::Fixed(10.0)))
                        .push(text("Webview not available. Choose how to open the widget:").size(14).color(color::GREY_3))
                        .push(Space::with_height(Length::Fixed(15.0)))
                        .push(
                            ui_button::primary(None, "Open in Browser")
                                .on_press(ViewMessage::MeldBuySell(MeldBuySellMessage::OpenWidget(widget_url.to_string())))
                                .width(Length::Fill)
                        )
                        .push(Space::with_height(Length::Fixed(10.0)))
                        .push(
                            ui_button::secondary(None, "Open in New Window")
                                .on_press(ViewMessage::MeldBuySell(MeldBuySellMessage::OpenWidgetInNewWindow(widget_url.to_string())))
                                .width(Length::Fill)
                        )
                        .push(Space::with_height(Length::Fixed(10.0)))
                        .push(
                            Container::new(
                                text(widget_url)
                                    .size(11)
                                    .color(color::BLUE)
                            )
                            .padding(10)
                            .style(theme::card::simple)
                            .width(Length::Fill)
                        )
                        .push(Space::with_height(Length::Fixed(10.0)))
                        .push(
                            ui_button::secondary(None, "Copy URL")
                                .on_press(ViewMessage::MeldBuySell(MeldBuySellMessage::CopyUrl(widget_url.to_string())))
                                .width(Length::Fill)
                        )
                        .align_x(Alignment::Center)
                )
                .width(Length::Fill)
                .height(Length::Fixed(350.0))
                .padding(20)
                .style(theme::card::simple)
                .into()
            }
        })
        .push(Space::with_height(Length::Fixed(15.0)))
        .push(
            ui_button::secondary(None, "Create Another Session")
                .on_press(ViewMessage::MeldBuySell(MeldBuySellMessage::ResetForm))
                .width(Length::Fill)
        )
        .align_x(Alignment::Center)
        .spacing(5)
        .max_width(500)
        .width(Length::Fill)
        .into()
}

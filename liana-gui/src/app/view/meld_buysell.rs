use iced::{
    widget::{text, Space},
    Alignment, Length,
};
use iced_webview::{advanced::WebView, Ultralight};

use liana::miniscript::bitcoin::Network;
use liana_ui::{
    color,
    component::{button as ui_button, form},
    icon::{bitcoin_icon, previous_icon},
    theme,
    widget::*,
};

use crate::app::view::{BuySellMessage, Message as ViewMessage};

pub struct ViewState {
    url: String,
    ready: bool,
}

pub struct BuySellPanel {
    pub wallet_address: form::Value<String>,
    pub country_code: form::Value<String>,
    pub source_amount: form::Value<String>,
    pub error: Option<String>,
    pub network: Network,

    // Ultralight webview component for Meld widget integration with performance optimizations
    pub webview: Option<WebView<Ultralight, crate::app::state::buysell::WebviewMessage>>,

    // Current webview page url
    pub session_url: Option<String>,

    // Current active webview "page": view_id
    pub active_page: Option<iced_webview::ViewId>,
}

impl BuySellPanel {
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

            error: None,
            network,

            webview: None,
            session_url: None,
            active_page: None,
        }
    }

    pub fn set_error(&mut self, error: String) {
        self.error = Some(error);
    }

    pub fn set_wallet_address(&mut self, address: String) {
        self.wallet_address.value = address;
        self.wallet_address.valid =
            !self.wallet_address.value.is_empty() && self.validate_wallet_address();
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
        self.source_amount.valid =
            !self.source_amount.value.is_empty() && self.source_amount.value.parse::<f64>().is_ok();
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

pub fn meld_buysell_view<'a>(state: &'a BuySellPanel) -> Element<'a, ViewMessage> {
    Container::new({
        // attempt to render webview
        let webview_widget = state
            .active_page
            .as_ref()
            .map(|v| {
                state.webview.as_ref().map(|s| {
                    s.view(*v)
                        .map(|a| ViewMessage::BuySell(BuySellMessage::WebviewAction(a)))
                })
            })
            .flatten();

        let column = match webview_widget {
            Some(webview) => Column::new().push(
                Row::new()
                    .push(
                        // Only show Previous button if we have a webview session active
                        Button::new(
                            Row::new()
                                .push(previous_icon().color(color::GREY_2))
                                .push(Space::with_width(Length::Fixed(5.0)))
                                .push(text("Previous").color(color::GREY_2))
                                .spacing(5)
                                .align_y(Alignment::Center),
                        )
                        .style(|_theme, _status| iced::widget::button::Style {
                            background: None,
                            text_color: color::GREY_2,
                            border: iced::Border::default(),
                            shadow: iced::Shadow::default(),
                        })
                        .on_press(ViewMessage::BuySell(BuySellMessage::CloseWebview)),
                    )
                    // Insert webview widget right after the Previous button if provided
                    .push(Space::with_width(Length::Fill))
                    .push(Space::with_height(Length::Fixed(20.0)))
                    .push(webview)
                    .align_y(Alignment::Center),
            ),
            // Only show form content if no session has been created (i.e., no webview is active)
            None => Column::new()
                .push(
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
                                                .padding(10),
                                        )
                                        .push(Space::with_width(Length::Fixed(15.0)))
                                        .push(
                                            Column::new()
                                                .push(
                                                    text("COINCUBE").size(16).color(color::ORANGE),
                                                )
                                                .push(
                                                    text("BUY/SELL").size(14).color(color::GREY_3),
                                                )
                                                .spacing(2),
                                        )
                                        .align_y(Alignment::Center),
                                )
                                .push(Space::with_width(Length::Fill))
                                .align_y(Alignment::Center),
                        ),
                )
                .push(Space::with_height(Length::Fixed(10.0)))
                .push(meld_form_content(state)),
        };

        column
            .align_x(Alignment::Center)
            .spacing(5) // Reduced spacing for more compact layout
            .max_width(600)
            .width(Length::Fill)
    })
    .padding(iced::Padding::new(2.0).left(40.0).right(40.0).bottom(20.0)) // further reduced padding for compact layout
    .center_x(Length::Fill)
    .into()
}

fn meld_form_content(state: &BuySellPanel) -> Element<'_, ViewMessage> {
    Column::new()
        .push_maybe(state.error.as_ref().map(|err| {
            Container::new(text(err).size(14).color(color::RED))
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
                        ViewMessage::BuySell(BuySellMessage::WalletAddressChanged(value))
                    })
                    .size(16)
                    .padding(15)
                })
                .spacing(5),
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
                                ViewMessage::BuySell(BuySellMessage::CountryCodeChanged(value))
                            })
                            .size(16)
                            .padding(15),
                        )
                        .spacing(5)
                        .width(Length::FillPortion(1)),
                )
                .push(Space::with_width(Length::Fixed(20.0)))
                .push(
                    Column::new()
                        .push(text("Amount (USD)").size(14).color(color::GREY_3))
                        .push(Space::with_height(Length::Fixed(5.0)))
                        .push(
                            form::Form::new_trimmed("60", &state.source_amount, |value| {
                                ViewMessage::BuySell(BuySellMessage::SourceAmountChanged(value))
                            })
                            .size(16)
                            .padding(15),
                        )
                        .spacing(5)
                        .width(Length::FillPortion(1)),
                ),
        )
        .push(Space::with_height(Length::Fixed(30.0)))
        .push(if state.active_page.is_some() {
            ui_button::secondary(None, "Creating Session...").width(Length::Fill)
        } else if state.is_form_valid() {
            ui_button::primary(None, "Create Widget Session")
                .on_press(ViewMessage::BuySell(BuySellMessage::CreateSession))
                .width(Length::Fill)
        } else {
            ui_button::secondary(None, "Create Widget Session").width(Length::Fill)
        })
        .align_x(Alignment::Center)
        .spacing(5)
        .max_width(500)
        .width(Length::Fill)
        .into()
}

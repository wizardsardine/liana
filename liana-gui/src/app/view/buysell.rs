#[cfg(all(feature = "dev-meld", feature = "dev-onramp"))]
compile_error!("`dev-meld` and `dev-onramp` should be exclusive");

use iced::{widget::Space, Alignment, Length};
#[cfg(feature = "webview")]
use iced_webview::{advanced::WebView, Ultralight};

use liana::miniscript::bitcoin::Network;
use liana_ui::{
    color,
    component::{button as ui_button, form},
    theme,
    widget::*,
};

#[cfg(feature = "dev-meld")]
use crate::app::buysell::meld::MeldClient;
use crate::app::view::{BuySellMessage, Message as ViewMessage};

pub struct BuySellPanel {
    pub wallet_address: form::Value<String>,
    #[cfg(feature = "dev-meld")]
    pub country_code: form::Value<String>,
    #[cfg(feature = "dev-onramp")]
    pub fiat_currency: form::Value<String>,
    pub source_amount: form::Value<String>,

    pub error: Option<String>,
    pub network: Network,

    #[cfg(feature = "dev-meld")]
    pub meld_client: MeldClient,

    // Ultralight webview component for Meld widget integration with performance optimizations
    #[cfg(feature = "webview")]
    pub webview: Option<WebView<Ultralight, crate::app::state::buysell::WebviewMessage>>,

    // Current webview page url
    #[cfg(feature = "webview")]
    pub session_url: Option<String>,

    // Current active webview "page": view_id
    #[cfg(feature = "webview")]
    pub active_page: Option<iced_webview::ViewId>,

    // Native login fields
    pub login_username: form::Value<String>,
    pub login_password: form::Value<String>,

    // Default build: account type selection state
    #[cfg(not(any(feature = "dev-meld", feature = "dev-onramp")))]
    pub selected_account_type: Option<crate::app::view::message::AccountType>,

    // Native flow current page
    #[cfg(not(any(feature = "dev-meld", feature = "dev-onramp")))]
    pub native_page: NativePage,

    // Registration fields (native flow)
    #[cfg(not(any(feature = "dev-meld", feature = "dev-onramp")))]
    pub first_name: form::Value<String>,
    #[cfg(not(any(feature = "dev-meld", feature = "dev-onramp")))]
    pub last_name: form::Value<String>,
    #[cfg(not(any(feature = "dev-meld", feature = "dev-onramp")))]
    pub email: form::Value<String>,
    #[cfg(not(any(feature = "dev-meld", feature = "dev-onramp")))]
    pub password1: form::Value<String>,
    #[cfg(not(any(feature = "dev-meld", feature = "dev-onramp")))]
    pub password2: form::Value<String>,
    #[cfg(not(any(feature = "dev-meld", feature = "dev-onramp")))]
    pub terms_accepted: bool,
}

#[cfg(not(any(feature = "dev-meld", feature = "dev-onramp")))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativePage {
    AccountSelect,
    Register,
}

impl BuySellPanel {
    pub fn new(network: Network) -> Self {
        Self {
            wallet_address: form::Value {
                value: "2N3oefVeg6stiTb5Kh3ozCSkaqmx91FDbsm".to_string(),
                warning: None,
                valid: true,
            },
            #[cfg(feature = "dev-meld")]
            country_code: form::Value {
                value: "US".to_string(),
                warning: None,
                valid: true,
            },
            #[cfg(feature = "dev-onramp")]
            fiat_currency: form::Value {
                value: "USD".to_string(),
                warning: None,
                valid: true,
            },
            source_amount: form::Value {
                value: "60".to_string(),
                warning: None,
                valid: true,
            },

            #[cfg(feature = "dev-meld")]
            meld_client: MeldClient::new(),
            #[cfg(not(any(feature = "dev-meld", feature = "dev-onramp")))]
            selected_account_type: None,
            #[cfg(not(any(feature = "dev-meld", feature = "dev-onramp")))]
            native_page: NativePage::AccountSelect,

            error: None,
            network,

            #[cfg(feature = "webview")]
            webview: None,
            #[cfg(feature = "webview")]
            session_url: None,
            #[cfg(feature = "webview")]
            active_page: None,

            login_username: form::Value {
                value: String::new(),
                warning: None,
                valid: false,
            },
            login_password: form::Value {
                value: String::new(),
                warning: None,
                valid: false,
            },

            // Native registration defaults
            #[cfg(not(any(feature = "dev-meld", feature = "dev-onramp")))]
            first_name: form::Value::default(),
            #[cfg(not(any(feature = "dev-meld", feature = "dev-onramp")))]
            last_name: form::Value::default(),
            #[cfg(not(any(feature = "dev-meld", feature = "dev-onramp")))]
            email: form::Value::default(),
            #[cfg(not(any(feature = "dev-meld", feature = "dev-onramp")))]
            password1: form::Value::default(),
            #[cfg(not(any(feature = "dev-meld", feature = "dev-onramp")))]
            password2: form::Value::default(),
            #[cfg(not(any(feature = "dev-meld", feature = "dev-onramp")))]
            terms_accepted: false,
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

    pub fn set_login_username(&mut self, v: String) {
        self.login_username.value = v;
        self.login_username.valid = !self.login_username.value.is_empty();
    }

    pub fn set_login_password(&mut self, v: String) {
        self.login_password.value = v;
        self.login_password.valid = !self.login_password.value.is_empty();
    }

    pub fn is_login_form_valid(&self) -> bool {
        self.login_username.valid && self.login_password.valid
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

    #[cfg(feature = "dev-meld")]
    pub fn set_country_code(&mut self, code: String) {
        self.country_code.value = code;
        self.country_code.valid = !self.country_code.value.is_empty();
    }

    #[cfg(feature = "dev-onramp")]
    pub fn set_fiat_currency(&mut self, code: String) {
        self.fiat_currency.value = code;
        // TODO: Check if currency is valid using static array
        self.fiat_currency.valid = true;
    }

    pub fn set_source_amount(&mut self, amount: String) {
        self.source_amount.value = amount;
        self.source_amount.valid =
            !self.source_amount.value.is_empty() && self.source_amount.value.parse::<f64>().is_ok();
    }

    pub fn is_form_valid(&self) -> bool {
        #[cfg(feature = "dev-meld")]
        #[allow(unused_variables)]
        let locale_check = self.country_code.valid && !self.country_code.value.is_empty();

        #[cfg(feature = "dev-onramp")]
        let locale_check = self.fiat_currency.valid && !self.fiat_currency.value.is_empty();

        #[cfg(not(any(feature = "dev-meld", feature = "dev-onramp")))]
        let locale_check = true;

        self.wallet_address.valid
            && locale_check
            && self.source_amount.valid
            && !self.wallet_address.value.is_empty()
            && !self.source_amount.value.is_empty()
    }

    pub fn view<'a>(&'a self) -> Container<'a, ViewMessage> {
        Container::new({
            // attempt to render webview (if available)
            #[cfg(feature = "webview")]
            let webview_widget = self
                .active_page
                .as_ref()
                .map(|v| {
                    self.webview.as_ref().map(|s| {
                        s.view(*v)
                            .map(|a| ViewMessage::BuySell(BuySellMessage::WebviewAction(a)))
                    })
                })
                .flatten();

            #[cfg(feature = "webview")]
            let column = match webview_widget {
                Some(w) => Column::new()
                    .push(
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
                            .align_y(Alignment::Center),
                    )
                    .push(Space::with_height(Length::Fixed(20.0)))
                    .push(
                        container(w)
                            .width(Length::Fixed(600.0))
                            .height(Length::Fixed(600.0)),
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
                                                Container::new(liana_ui::icon::bitcoin_icon().size(24))
                                                    .style(theme::container::border)
                                                    .padding(10),
                                            )
                                            .push(Space::with_width(Length::Fixed(15.0)))
                                            .push(
                                                Column::new()
                                                    .push(
                                                        text("COINCUBE")
                                                            .size(16)
                                                            .color(color::ORANGE),
                                                    )
                                                    .push(
                                                        text("BUY/SELL")
                                                            .size(14)
                                                            .color(color::GREY_3),
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
                    .push(self.form_view()),
            };

            #[cfg(not(feature = "webview"))]
            let column = Column::new().push(self.form_view());

            column
                .align_x(Alignment::Center)
                .spacing(5) // Reduced spacing for more compact layout
                .max_width(600)
                .width(Length::Fill)
        })
        .padding(iced::Padding::new(2.0).left(40.0).right(40.0).bottom(20.0)) // further reduced padding for compact layout
        .center_x(Length::Fill)
    }

    #[cfg(not(any(feature = "dev-meld", feature = "dev-onramp")))]
    fn form_view<'a>(&'a self) -> Column<'a, ViewMessage> {
        match self.native_page {
            NativePage::AccountSelect => self.native_login_form(),
            NativePage::Register => self.native_register_form(),
        }
    }

    #[cfg(any(feature = "dev-meld", feature = "dev-onramp"))]
    fn form_view<'a>(&'a self) -> Column<'a, ViewMessage> {
        Column::new()
            .push_maybe(self.error.as_ref().map(|err| {
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
                        let placeholder = match self.network {
                            Network::Bitcoin => "Enter mainnet Bitcoin address (1, 3, bc1)",
                            _ => "Enter testnet Bitcoin address (2, tb1, bcrt1)",
                        };

                        form::Form::new_trimmed(placeholder, &self.wallet_address, |value| {
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
                            .push(
                                if cfg!(feature = "dev-onramp") {
                                    text("Fiat Currency")
                                } else {
                                    text("Country Code")
                                }
                                .size(14)
                                .color(color::GREY_3),
                            )
                            .push(Space::with_height(Length::Fixed(5.0)))
                            .push({
                                #[cfg(feature = "dev-meld")]
                                {
                                    form::Form::new_trimmed("US", &self.country_code, |value| {
                                        ViewMessage::BuySell(BuySellMessage::CountryCodeChanged(
                                            value,
                                        ))
                                    })
                                    .size(16)
                                    .padding(15)
                                }

                                #[cfg(feature = "dev-onramp")]
                                {
                                    form::Form::new_trimmed("USD", &self.fiat_currency, |value| {
                                        ViewMessage::BuySell(BuySellMessage::FiatCurrencyChanged(
                                            value,
                                        ))
                                    })
                                    .size(16)
                                    .padding(15)
                                }
                            })
                            .spacing(5)
                            .width(Length::FillPortion(1)),
                    )
                    .push(Space::with_width(Length::Fixed(20.0)))
                    .push(
                        Column::new()
                            .push(text("Fiat Amount").size(14).color(color::GREY_3))
                            .push(Space::with_height(Length::Fixed(5.0)))
                            .push(
                                form::Form::new_trimmed("60", &self.source_amount, |value| {
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
            .push(if self.active_page.is_some() {
                ui_button::secondary(Some(liana_ui::icon::globe_icon()), "Creating Session...")
                    .width(Length::Fill)
            } else if self.is_form_valid() {
                ui_button::primary(Some(liana_ui::icon::globe_icon()), "Create Widget Session")
                    .on_press(ViewMessage::BuySell(BuySellMessage::CreateSession))
                    .width(Length::Fill)
            } else {
                ui_button::secondary(Some(liana_ui::icon::globe_icon()), "Create Widget Session")
                    .width(Length::Fill)
            })
            .align_x(Alignment::Center)
            .spacing(5)
            .max_width(500)
            .width(Length::Fill)
    }
}

#[cfg(not(any(feature = "dev-meld", feature = "dev-onramp")))]
impl BuySellPanel {
    fn native_login_form<'a>(&'a self) -> Column<'a, ViewMessage> {
        use liana_ui::component::card as ui_card;
        use liana_ui::component::text as ui_text;
        use liana_ui::icon::{building_icon, person_icon};

        let header = Row::new()
            .push(
                Row::new()
                    .push(ui_text::h4_bold("COIN").color(color::ORANGE))
                    .push(ui_text::h4_bold("CUBE").color(color::WHITE))
                    .spacing(0),
            )
            .push(Space::with_width(Length::Fixed(8.0)))
            .push(ui_text::h5_regular("BUY/SELL").color(color::GREY_3));

        let subheader = ui_text::p1_regular("Choose your account type").color(color::WHITE);

        let is_individual = matches!(
            self.selected_account_type,
            Some(crate::app::view::message::AccountType::Individual)
        );
        let is_business = matches!(
            self.selected_account_type,
            Some(crate::app::view::message::AccountType::Business)
        );

        let make_card = |title: &str,
                         desc: &str,
                         icon: Element<'a, ViewMessage>,
                         selected: bool,
                         on_press: ViewMessage|
         -> Element<'a, ViewMessage> {
            let content = Row::new()
                .spacing(12)
                .align_y(Alignment::Center)
                .push(Container::new(icon).width(Length::Fixed(28.0)))
                .push(
                    Column::new()
                        .push(ui_text::p1_bold(title).color(color::WHITE))
                        .push(ui_text::p2_regular(desc).color(color::GREY_3))
                        .spacing(2),
                );
            let card_body = ui_card::simple(content)
                .style(if selected {
                    theme::card::warning
                } else {
                    theme::card::border
                })
                .width(Length::Fill)
                .height(Length::Fixed(84.0));
            if selected {
                card_body.into()
            } else {
                iced::widget::button(card_body)
                    .style(theme::button::transparent_border)
                    .on_press(on_press)
                    .into()
            }
        };

        let individual = make_card(
            "Individual",
            "For individuals who want to buy and manage Bitcoin",
            Element::from(Container::new(person_icon())),
            is_individual,
            ViewMessage::BuySell(BuySellMessage::AccountTypeSelected(
                crate::app::view::message::AccountType::Individual,
            )),
        );
        let business = make_card(
            "Business",
            "For LLCs, trusts, corporations, partnerships, and more who want to buy and manage Bitcoin.",
            Element::from(Container::new(building_icon())),
            is_business,
            ViewMessage::BuySell(BuySellMessage::AccountTypeSelected(crate::app::view::message::AccountType::Business)),
        );

        let button = if self.selected_account_type.is_some() {
            ui_button::primary(None, "Get Started")
                .on_press(ViewMessage::BuySell(BuySellMessage::GetStarted))
                .width(Length::Fill)
        } else {
            ui_button::secondary(None, "Get Started").width(Length::Fill)
        };

        Column::new()
            .push(header)
            .push(Space::with_height(Length::Fixed(10.0)))
            .push(subheader)
            .push(Space::with_height(Length::Fixed(20.0)))
            .push(individual)
            .push(Space::with_height(Length::Fixed(10.0)))
            .push(business)
            .push(Space::with_height(Length::Fixed(30.0)))
            .push(button)
            .align_x(Alignment::Center)
            .spacing(5)
            .max_width(500)
            .width(Length::Fill)
    }
}

#[cfg(not(any(feature = "dev-meld", feature = "dev-onramp")))]
impl BuySellPanel {
    fn native_register_form<'a>(&'a self) -> Column<'a, ViewMessage> {
        use liana_ui::component::text as ui_text;
        use liana_ui::component::text::text;
        use liana_ui::icon::{globe_icon, previous_icon};
        use liana_ui::component::button as ui_button;
        use iced::widget::checkbox;

        // Top bar with previous
        let top_bar = Row::new()
            .push(
                Button::new(
                    Row::new()
                        .push(previous_icon().color(color::GREY_2))
                        .push(Space::with_width(Length::Fixed(5.0)))
                        .push(text("Previous").color(color::GREY_2))
                        .spacing(5)
                        .align_y(Alignment::Center),
                )
                .style(|_, _| iced::widget::button::Style {
                    background: None,
                    text_color: color::GREY_2,
                    border: iced::Border::default(),
                    shadow: iced::Shadow::default(),
                })
                .on_press(ViewMessage::Previous),
            )
            .align_y(Alignment::Center);

        // Brand header
        let brand = Row::new()
            .push(Space::with_width(Length::Fill))
            .push(
                Row::new()
                    .push(ui_text::h4_bold("COIN").color(color::ORANGE))
                    .push(ui_text::h4_bold("CUBE").color(color::WHITE))
                    .push(Space::with_width(Length::Fixed(8.0)))
                    .push(ui_text::h5_regular("BUY/SELL").color(color::GREY_3))
                    .spacing(0)
                    .align_y(Alignment::Center),
            )
            .push(Space::with_width(Length::Fill))
            .align_y(Alignment::Center);

        // Title and subtitle
        let title = Column::new()
            .push(ui_text::h3("Create an Account").color(color::WHITE))
            .push(
                ui_text::p2_regular(
                    "Get started with your personal Bitcoin wallet. Buy, store, and manage crypto securely, all in one place.",
                )
                .color(color::GREY_3),
            )
            .spacing(10)
            .align_x(Alignment::Center);

        // Continue with Google (placeholder)
        let google = ui_button::secondary(Some(globe_icon()), "Continue with Google").width(Length::Fill);

        // Divider "Or"
        let divider = Row::new()
            .push(Container::new(Space::with_height(Length::Fixed(1.0))).width(Length::Fill))
            .push(text("  Or  ").color(color::GREY_3))
            .push(Container::new(Space::with_height(Length::Fixed(1.0))).width(Length::Fill));

        let name_row = Row::new()
            .push(
                Container::new(
                    form::Form::new("First Name", &self.first_name, |v| {
                        ViewMessage::BuySell(BuySellMessage::FirstNameChanged(v))
                    })
                    .size(16)
                    .padding(15),
                )
                .width(Length::FillPortion(1)),
            )
            .push(Space::with_width(Length::Fixed(12.0)))
            .push(
                Container::new(
                    form::Form::new("Last Name", &self.last_name, |v| {
                        ViewMessage::BuySell(BuySellMessage::LastNameChanged(v))
                    })
                    .size(16)
                    .padding(15),
                )
                .width(Length::FillPortion(1)),
            );

        let email = form::Form::new("Email Address", &self.email, |v| {
            ViewMessage::BuySell(BuySellMessage::EmailChanged(v))
        })
        .size(16)
        .padding(15);

        let password = form::Form::new("Password", &self.password1, |v| {
            ViewMessage::BuySell(BuySellMessage::Password1Changed(v))
        })
        .size(16)
        .padding(15)
        .secure();

        let confirm = form::Form::new("Confirm Password", &self.password2, |v| {
            ViewMessage::BuySell(BuySellMessage::Password2Changed(v))
        })
        .size(16)
        .padding(15)
        .secure();

        let terms = Row::new()
            .push(
                checkbox("", self.terms_accepted).on_toggle(|b| {
                    ViewMessage::BuySell(BuySellMessage::TermsToggled(b))
                }),
            )
            .push(Space::with_width(Length::Fixed(8.0)))
            .push(
                Row::new()
                    .push(ui_text::p2_regular("I agree to COINCUBE's ").color(color::GREY_3))
                    .push(ui_text::p2_regular("Terms of Service").color(color::ORANGE))
                    .push(ui_text::p2_regular(" and ").color(color::GREY_3))
                    .push(ui_text::p2_regular("Privacy Policy").color(color::ORANGE)),
            )
            .align_y(Alignment::Center);

        let create_btn = if self.is_registration_valid() {
            ui_button::primary(None, "Create Account")
                .on_press(ViewMessage::BuySell(BuySellMessage::SubmitRegistration))
                .width(Length::Fill)
        } else {
            ui_button::secondary(None, "Create Account").width(Length::Fill)
        };

        Column::new()
            .push(top_bar)
            .push(Space::with_height(Length::Fixed(10.0)))
            .push(brand)
            .push(Space::with_height(Length::Fixed(30.0)))
            .push(title)
            .push(Space::with_height(Length::Fixed(20.0)))
            .push(google)
            .push(Space::with_height(Length::Fixed(10.0)))
            .push(divider)
            .push(Space::with_height(Length::Fixed(10.0)))
            .push(name_row)
            .push(Space::with_height(Length::Fixed(10.0)))
            .push(email)
            .push(Space::with_height(Length::Fixed(10.0)))
            .push(password)
            .push(Space::with_height(Length::Fixed(10.0)))
            .push(confirm)
            .push(Space::with_height(Length::Fixed(10.0)))
            .push(terms)
            .push(Space::with_height(Length::Fixed(20.0)))
            .push(create_btn)
            .align_x(Alignment::Center)
            .spacing(5)
            .max_width(500)
            .width(Length::Fill)
    }

    #[inline]
    fn is_registration_valid(&self) -> bool {
        let email_ok = self.email.value.contains('@') && self.email.value.contains('.');
        let pw_ok = self.password1.value.len() >= 8 && self.password1.value == self.password2.value;
        !self.first_name.value.is_empty()
            && !self.last_name.value.is_empty()
            && email_ok
            && pw_ok
            && self.terms_accepted
    }
}

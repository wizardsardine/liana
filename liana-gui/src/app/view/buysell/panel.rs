#![allow(unused_imports)]

use iced::{
    widget::{container, Space},
    Alignment, Length,
};
use iced_webview::{advanced::WebView, Ultralight};

use liana::miniscript::bitcoin::Network;
use liana_ui::{
    color,
    component::{button as ui_button, form, text::text},
    icon::*,
    theme,
    widget::*,
};

use crate::app::view::{BuySellMessage, Message as ViewMessage};
use super::flow_state::{BuySellFlowState, AfricaFlowState, InternationalFlowState, NativePage};

pub struct BuySellPanel {
    // Common fields (always present)
    pub wallet_address: form::Value<String>,
    pub source_amount: form::Value<String>,
    pub error: Option<String>,
    pub network: Network,

    // Geolocation detection state
    pub detected_region: Option<String>,
    pub detected_country: Option<String>,
    pub region_detection_failed: bool,

    // Webview (used by international flow)
    pub webview: Option<WebView<Ultralight, crate::app::state::buysell::WebviewMessage>>,
    pub session_url: Option<String>,
    pub active_page: Option<iced_webview::ViewId>,

    // Runtime state - determines which flow is active
    pub flow_state: BuySellFlowState,
}

impl BuySellPanel {
    pub fn new(network: Network) -> Self {
        Self {
            wallet_address: form::Value {
                value: "2N3oefVeg6stiTb5Kh3ozCSkaqmx91FDbsm".to_string(),
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
            detected_region: None,
            detected_country: None,
            region_detection_failed: false,
            webview: None,
            session_url: None,
            active_page: None,
            // Start in detecting state
            flow_state: BuySellFlowState::DetectingRegion,
        }
    }

    /// Handle geolocation result and transition to appropriate flow state
    pub fn handle_region_detected(&mut self, region: &str, country: String) {
        self.detected_region = Some(region.to_string());
        self.detected_country = Some(country);
        self.error = None;

        self.flow_state = match region {
            "africa" => BuySellFlowState::Africa(AfricaFlowState::new()),
            "international" => BuySellFlowState::International(InternationalFlowState::new()),
            _ => BuySellFlowState::DetectionFailed,
        };
    }

    // Helper methods to access Africa flow state fields (for backward compatibility during migration)
    pub fn africa_state(&self) -> Option<&AfricaFlowState> {
        if let BuySellFlowState::Africa(ref state) = self.flow_state {
            Some(state)
        } else {
            None
        }
    }

    pub fn africa_state_mut(&mut self) -> Option<&mut AfricaFlowState> {
        if let BuySellFlowState::Africa(ref mut state) = self.flow_state {
            Some(state)
        } else {
            None
        }
    }

    pub fn international_state(&self) -> Option<&InternationalFlowState> {
        if let BuySellFlowState::International(ref state) = self.flow_state {
            Some(state)
        } else {
            None
        }
    }

    pub fn international_state_mut(&mut self) -> Option<&mut InternationalFlowState> {
        if let BuySellFlowState::International(ref mut state) = self.flow_state {
            Some(state)
        } else {
            None
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
        if let BuySellFlowState::Africa(ref mut state) = self.flow_state {
            state.login_username.value = v;
            state.login_username.valid = !state.login_username.value.is_empty();
        }
    }

    pub fn set_login_password(&mut self, v: String) {
        if let BuySellFlowState::Africa(ref mut state) = self.flow_state {
            state.login_password.value = v;
            state.login_password.valid = !state.login_password.value.is_empty();
        }
    }

    pub fn is_login_form_valid(&self) -> bool {
        if let BuySellFlowState::Africa(ref state) = self.flow_state {
            state.login_username.valid && state.login_password.valid
        } else {
            false
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

    pub fn set_source_amount(&mut self, amount: String) {
        self.source_amount.value = amount;
        self.source_amount.valid =
            !self.source_amount.value.is_empty() && self.source_amount.value.parse::<f64>().is_ok();
    }

    pub fn is_form_valid(&self) -> bool {
        self.wallet_address.valid
            && self.source_amount.valid
            && !self.wallet_address.value.is_empty()
            && !self.source_amount.value.is_empty()
    }

    pub fn view<'a>(&'a self) -> Container<'a, ViewMessage> {
        Container::new({
            // attempt to render webview (if available)
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
                                                Container::new(
                                                    liana_ui::icon::bitcoin_icon().size(24),
                                                )
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

            // webview always enabled by default
            // let column = Column::new().push(self.form_view());

            column
                .align_x(Alignment::Center)
                .spacing(5) // Reduced spacing for more compact layout
                .max_width(600)
                .width(Length::Fill)
        })
        .padding(iced::Padding::new(2.0).left(40.0).right(40.0).bottom(20.0)) // further reduced padding for compact layout
        .center_x(Length::Fill)
    }

    fn form_view<'a>(&'a self) -> Column<'a, ViewMessage> {
        match &self.flow_state {
            BuySellFlowState::DetectingRegion => self.render_loading(),
            BuySellFlowState::Africa(state) => self.render_africa_flow(state),
            BuySellFlowState::International(_state) => self.provider_selection_form(),
            BuySellFlowState::DetectionFailed => self.provider_selection_form(),
        }
    }

    fn render_loading<'a>(&'a self) -> Column<'a, ViewMessage> {
        Column::new()
            .push(Space::with_height(Length::Fixed(50.0)))
            .push(text("Detecting your region...").size(16).color(color::GREY_3))
            .push(Space::with_height(Length::Fixed(20.0)))
            .align_x(Alignment::Center)
            .spacing(10)
    }

    fn render_africa_flow<'a>(&'a self, state: &'a AfricaFlowState) -> Column<'a, ViewMessage> {
        match state.native_page {
            NativePage::AccountSelect => self.native_account_select_form(state),
            NativePage::Login => self.native_login_form(state),
            NativePage::Register => self.native_register_form(state),
            NativePage::VerifyEmail => self.native_verify_email_form(state),
            NativePage::CoincubePay => self.coincube_pay_form(state),
        }
    }

    fn provider_selection_form<'a>(&'a self) -> Column<'a, ViewMessage> {
        use liana_ui::component::{button as ui_button, text as ui_text};
        let info = if let Some(country) = &self.detected_country {
            format!("International region detected (country: {}). Choose a provider:", country)
        } else {
            "Choose a provider:".to_string()
        };
        Column::new()
            .push_maybe(self.error.as_ref().map(|err| {
                Container::new(text(err).size(14).color(color::RED))
                    .padding(10)
                    .style(theme::card::simple)
            }))
            .push(Space::with_height(Length::Fixed(10.0)))
            .push(ui_text::p1_bold(&info).color(color::WHITE))
            .push(Space::with_height(Length::Fixed(15.0)))
            .push(
                ui_button::primary(None, "Continue with Meld")
                    .on_press(ViewMessage::BuySell(BuySellMessage::OpenMeld))
                    .width(Length::Fill),
            )
            .push(Space::with_height(Length::Fixed(10.0)))
            .push(
                ui_button::primary(None, "Continue with Onramper")
                    .on_press(ViewMessage::BuySell(BuySellMessage::OpenOnramper))
                    .width(Length::Fill),
            )
            .align_x(Alignment::Center)
            .spacing(10)
            .max_width(500)
            .width(Length::Fill)
    }

}

impl BuySellPanel {
    fn native_account_select_form<'a>(&'a self, state: &'a AfricaFlowState) -> Column<'a, ViewMessage> {
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
            state.selected_account_type,
            Some(crate::app::view::message::AccountType::Individual)
        );
        let is_business = matches!(
            state.selected_account_type,
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

        let button = if state.selected_account_type.is_some() {
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

    fn native_login_form<'a>(&'a self, state: &'a AfricaFlowState) -> Column<'a, ViewMessage> {
        use liana_ui::component::card as ui_card;
        use liana_ui::component::form;
        use liana_ui::component::text as ui_text;

        let header = Row::new()
            .push(
                Row::new()
                    .push(ui_text::h4_bold("COIN").color(color::ORANGE))
                    .push(ui_text::h4_bold("CUBE").color(color::WHITE))
                    .spacing(0),
            )
            .push(Space::with_width(Length::Fixed(8.0)))
            .push(ui_text::h5_regular("BUY/SELL").color(color::GREY_3));

        let subheader = ui_text::p1_regular("Sign in to your account").color(color::WHITE);

        let email_input = form::Form::new_trimmed("Email", &state.login_username, |v| {
            ViewMessage::BuySell(BuySellMessage::LoginUsernameChanged(v))
        })
        .warning("Please enter your email address");

        let password_input = form::Form::new_trimmed("Password", &state.login_password, |v| {
            ViewMessage::BuySell(BuySellMessage::LoginPasswordChanged(v))
        })
        .warning("Please enter your password")
        .secure();

        let login_button = if self.is_login_form_valid() {
            ui_button::primary(None, "Sign In")
                .on_press(ViewMessage::BuySell(BuySellMessage::SubmitLogin))
                .width(Length::Fill)
        } else {
            ui_button::secondary(None, "Sign In").width(Length::Fill)
        };

        let create_account_link = iced::widget::button(
            ui_text::p2_regular("Don't have an account? Sign up").color(color::ORANGE),
        )
        .style(theme::button::transparent)
        .on_press(ViewMessage::BuySell(BuySellMessage::CreateAccountPressed));

        Column::new()
            .push(header)
            .push(Space::with_height(Length::Fixed(10.0)))
            .push(subheader)
            .push(Space::with_height(Length::Fixed(30.0)))
            .push(email_input)
            .push(Space::with_height(Length::Fixed(20.0)))
            .push(password_input)
            .push(Space::with_height(Length::Fixed(30.0)))
            .push(login_button)
            .push(Space::with_height(Length::Fixed(20.0)))
            .push(create_account_link)
            .align_x(Alignment::Center)
            .spacing(5)
            .max_width(500)
            .width(Length::Fill)
    }
}

#[cfg(not(any(feature = "dev-meld", feature = "dev-onramp")))]
impl BuySellPanel {
    fn native_register_form<'a>(&'a self, state: &'a AfricaFlowState) -> Column<'a, ViewMessage> {
        use iced::widget::checkbox;
        use liana_ui::component::button as ui_button;
        use liana_ui::component::text as ui_text;
        use liana_ui::component::text::text;
        use liana_ui::icon::{globe_icon, previous_icon};

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
        let google =
            ui_button::secondary(Some(globe_icon()), "Continue with Google").width(Length::Fill);

        // Divider "Or"
        let divider = Row::new()
            .push(Container::new(Space::with_height(Length::Fixed(1.0))).width(Length::Fill))
            .push(text("  Or  ").color(color::GREY_3))
            .push(Container::new(Space::with_height(Length::Fixed(1.0))).width(Length::Fill));

        let name_row = Row::new()
            .push(
                Container::new(
                    form::Form::new("First Name", &state.first_name, |v| {
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
                    form::Form::new("Last Name", &state.last_name, |v| {
                        ViewMessage::BuySell(BuySellMessage::LastNameChanged(v))
                    })
                    .size(16)
                    .padding(15),
                )
                .width(Length::FillPortion(1)),
            );

        let email = form::Form::new("Email Address", &state.email, |v| {
            ViewMessage::BuySell(BuySellMessage::EmailChanged(v))
        })
        .size(16)
        .padding(15);

        let password = form::Form::new("Password", &state.password1, |v| {
            ViewMessage::BuySell(BuySellMessage::Password1Changed(v))
        })
        .size(16)
        .padding(15)
        .secure();

        let confirm = form::Form::new("Confirm Password", &state.password2, |v| {
            ViewMessage::BuySell(BuySellMessage::Password2Changed(v))
        })
        .size(16)
        .padding(15)
        .secure();

        let terms = Row::new()
            .push(
                checkbox("", state.terms_accepted)
                    .on_toggle(|b| ViewMessage::BuySell(BuySellMessage::TermsToggled(b))),
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

        let create_btn = if self.is_registration_valid(state) {
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
            .push_maybe(self.get_password_validation_message(state).map(|msg| {
                Container::new(ui_text::p2_regular(&msg).color(color::RED))
                    .padding(iced::Padding::new(2.0).top(2.0))
            }))
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
    pub fn is_registration_valid(&self, state: &AfricaFlowState) -> bool {
        let email_ok = state.email.value.contains('@') && state.email.value.contains('.');
        let pw_ok = self.is_password_valid(state) && state.password1.value == state.password2.value;
        !state.first_name.value.is_empty()
            && !state.last_name.value.is_empty()
            && email_ok
            && pw_ok
            && state.terms_accepted
    }

    #[inline]
    pub fn is_password_valid(&self, state: &AfricaFlowState) -> bool {
        let password = &state.password1.value;
        if password.len() < 8 {
            return false;
        }

        let has_upper = password.chars().any(|c| c.is_ascii_uppercase());
        let has_lower = password.chars().any(|c| c.is_ascii_lowercase());
        let has_digit = password.chars().any(|c| c.is_ascii_digit());
        let has_special = password.chars().any(|c| !c.is_ascii_alphanumeric());

        has_upper && has_lower && has_digit && has_special
    }

    pub fn get_password_validation_message(&self, state: &AfricaFlowState) -> Option<String> {
        let password = &state.password1.value;
        if password.is_empty() {
            return None;
        }

        let mut issues = Vec::new();

        if password.len() < 8 {
            issues.push("at least 8 characters");
        }
        if !password.chars().any(|c| c.is_ascii_uppercase()) {
            issues.push("1 uppercase letter");
        }
        if !password.chars().any(|c| c.is_ascii_lowercase()) {
            issues.push("1 lowercase letter");
        }
        if !password.chars().any(|c| c.is_ascii_digit()) {
            issues.push("1 number");
        }
        if !password.chars().any(|c| !c.is_ascii_alphanumeric()) {
            issues.push("1 special character");
        }

        if issues.is_empty() {
            None
        } else {
            Some(format!("Password must contain: {}", issues.join(", ")))
        }
    }

    pub fn set_email_verification_status(&mut self, verified: Option<bool>) {
        if let BuySellFlowState::Africa(ref mut state) = self.flow_state {
            state.email_verification_status = verified;
        }
    }

    fn native_verify_email_form<'a>(&'a self, state: &'a AfricaFlowState) -> Column<'a, ViewMessage> {
        use liana_ui::component::button as ui_button;
        use liana_ui::component::text as ui_text;
        use liana_ui::component::text::text;
        use liana_ui::icon::{check_icon, previous_icon, reload_icon};

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

        // Title and status-dependent subtitle
        let title = match state.email_verification_status {
            Some(true) => Column::new()
                .push(ui_text::h3("Email Verified!").color(color::GREEN))
                .push(
                    ui_text::p2_regular(
                        "Your email has been successfully verified. You can now continue.",
                    )
                    .color(color::GREY_3),
                )
                .spacing(10)
                .align_x(Alignment::Center),
            _ => Column::new()
                .push(ui_text::h3("Verify Your Email").color(color::WHITE))
                .push(
                    ui_text::p2_regular("We've sent a verification email to your account.")
                        .color(color::GREY_3),
                )
                .push(
                    ui_text::p2_regular(
                        "Check your inbox and click the verification link to continue.",
                    )
                    .color(color::GREY_3),
                )
                .spacing(10)
                .align_x(Alignment::Center),
        };

        // Email display
        let email_display = Column::new()
            .push(
                ui_text::p2_regular(&format!("Email sent to: {}", state.email.value))
                    .color(color::WHITE),
            )
            .spacing(10)
            .align_x(Alignment::Center);

        // Status indicator and instructions
        let status_section = match state.email_verification_status {
            None => Column::new()
                .push(ui_text::p2_regular("Checking verification status...").color(color::ORANGE))
                .spacing(10)
                .align_x(Alignment::Center),
            Some(true) => Column::new()
                .push(
                    Row::new()
                        .push(check_icon().color(color::GREEN))
                        .push(Space::with_width(Length::Fixed(8.0)))
                        .push(ui_text::p1_bold("Email verified successfully!").color(color::GREEN))
                        .align_y(Alignment::Center),
                )
                .spacing(10)
                .align_x(Alignment::Center),
            Some(false) => Column::new()
                .push(ui_text::p2_regular("Waiting for email verification...").color(color::GREY_3))
                .push(
                    ui_text::p2_regular("Click the link in your email to verify your account.")
                        .color(color::GREY_3),
                )
                .spacing(10)
                .align_x(Alignment::Center),
        };

        // Action buttons
        let action_buttons = match state.email_verification_status {
            Some(true) => Row::new()
                .push(
                    ui_button::primary(None, "Continue")
                        .on_press(ViewMessage::Next) // This would proceed to next step
                        .width(Length::Fill),
                )
                .spacing(10),
            _ => Row::new()
                .push(
                    ui_button::secondary(Some(reload_icon()), "Check Status")
                        .on_press(ViewMessage::BuySell(
                            BuySellMessage::CheckEmailVerificationStatus,
                        ))
                        .width(Length::FillPortion(1)),
                )
                .push(Space::with_width(Length::Fixed(10.0)))
                .push(
                    ui_button::link(None, "Resend Email").on_press(ViewMessage::BuySell(
                        BuySellMessage::ResendVerificationEmail,
                    )),
                )
                .spacing(10)
                .align_y(Alignment::Center),
        };

        Column::new()
            .push(top_bar)
            .push(Space::with_height(Length::Fixed(10.0)))
            .push(brand)
            .push(Space::with_height(Length::Fixed(30.0)))
            .push(title)
            .push(Space::with_height(Length::Fixed(30.0)))
            .push(email_display)
            .push(Space::with_height(Length::Fixed(20.0)))
            .push(status_section)
            .push(Space::with_height(Length::Fixed(30.0)))
            .push(action_buttons)
            .align_x(Alignment::Center)
            .spacing(5)
            .max_width(500)
            .width(Length::Fill)
    }

    #[cfg(not(any(feature = "dev-meld", feature = "dev-onramp")))]
    fn coincube_pay_form<'a>(&'a self, state: &'a AfricaFlowState) -> Column<'a, ViewMessage> {
        use liana_ui::component::{button as ui_button, text as ui_text};

        let header = Row::new()
            .push(
                Row::new()
                    .push(ui_text::h4_bold("COINCUBE").color(color::ORANGE))
                    .push(ui_text::h4_bold(" PAY").color(color::WHITE))
                    .spacing(0),
            )
            .push(Space::with_width(Length::Fixed(8.0)))
            .push(ui_text::h5_regular("Bitcoin ↔ Fiat Exchange").color(color::GREY_3));

        let mut column = Column::new()
            .push(header)
            .push(Space::with_height(Length::Fixed(20.0)));

        // Error display
        if let Some(error) = &self.error {
            column = column
                .push(
                    Container::new(text(error).size(14).color(color::RED))
                        .padding(10)
                        .style(theme::card::invalid),
                )
                .push(Space::with_height(Length::Fixed(10.0)));
        }

        // Current price display
        if let Some(price) = &state.mavapay_current_price {
            column = column
                .push(
                    Container::new(
                        Row::new()
                            .push(bitcoin_icon().size(20).color(color::ORANGE))
                            .push(Space::with_width(Length::Fixed(10.0)))
                            .push(
                                text(format!("1 BTC = {:.2} {}", price.price, price.currency))
                                    .size(16)
                                    .color(color::WHITE),
                            )
                            .align_y(Alignment::Center),
                    )
                    .padding(15)
                    .style(theme::card::simple),
                )
                .push(Space::with_height(Length::Fixed(15.0)));
        }

        // Quick actions
        let actions_row = Row::new()
            .push(
                ui_button::primary(None, "Get Price")
                    .on_press(ViewMessage::BuySell(BuySellMessage::MavapayGetPrice))
                    .width(Length::Fixed(120.0)),
            )
            .push(Space::with_width(Length::Fixed(10.0)))
            .push(
                ui_button::secondary(None, "View Transactions")
                    .on_press(ViewMessage::BuySell(BuySellMessage::MavapayGetTransactions))
                    .width(Length::Fixed(150.0)),
            )
            .spacing(10);

        column = column
            .push(actions_row)
            .push(Space::with_height(Length::Fixed(20.0)));

        // Exchange form
        let exchange_form = Container::new(
            Column::new()
                .push(
                    Row::new()
                        .push(text("💱").size(20))
                        .push(Space::with_width(Length::Fixed(10.0)))
                        .push(ui_text::h5_medium("Create Exchange Quote").color(color::WHITE))
                        .align_y(Alignment::Center),
                )
                .push(Space::with_height(Length::Fixed(15.0)))
                .push(
                    Row::new()
                        .push(
                            Column::new()
                                .push(text("Amount").size(14).color(color::GREY_3))
                                .push(Space::with_height(Length::Fixed(5.0)))
                                .push(
                                    form::Form::new_trimmed(
                                        "100000",
                                        &state.mavapay_amount,
                                        |value| {
                                            ViewMessage::BuySell(
                                                BuySellMessage::MavapayAmountChanged(value),
                                            )
                                        },
                                    )
                                    .size(14)
                                    .padding(10),
                                )
                                .width(Length::Fixed(120.0)),
                        )
                        .push(Space::with_width(Length::Fixed(10.0)))
                        .push(
                            Column::new()
                                .push(text("From").size(14).color(color::GREY_3))
                                .push(Space::with_height(Length::Fixed(5.0)))
                                .push(
                                    form::Form::new_trimmed(
                                        "BTCSAT",
                                        &state.mavapay_source_currency,
                                        |value| {
                                            ViewMessage::BuySell(
                                                BuySellMessage::MavapaySourceCurrencyChanged(value),
                                            )
                                        },
                                    )
                                    .size(14)
                                    .padding(10),
                                )
                                .width(Length::Fixed(100.0)),
                        )
                        .push(Space::with_width(Length::Fixed(10.0)))
                        .push(
                            Column::new()
                                .push(text("To").size(14).color(color::GREY_3))
                                .push(Space::with_height(Length::Fixed(5.0)))
                                .push(
                                    form::Form::new_trimmed(
                                        "NGNKOBO",
                                        &state.mavapay_target_currency,
                                        |value| {
                                            ViewMessage::BuySell(
                                                BuySellMessage::MavapayTargetCurrencyChanged(value),
                                            )
                                        },
                                    )
                                    .size(14)
                                    .padding(10),
                                )
                                .width(Length::Fixed(100.0)),
                        )
                        .spacing(10),
                )
                .push(Space::with_height(Length::Fixed(15.0)))
                // Bank Account Details Section
                .push(text("Bank Account Details").size(16).color(color::WHITE))
                .push(Space::with_height(Length::Fixed(10.0)))
                .push(
                    Row::new()
                        .push(
                            Column::new()
                                .push(text("Account Number").size(14).color(color::GREY_3))
                                .push(Space::with_height(Length::Fixed(5.0)))
                                .push(
                                    form::Form::new_trimmed(
                                        "1234567890",
                                        &state.mavapay_bank_account_number,
                                        |value| {
                                            ViewMessage::BuySell(
                                                BuySellMessage::MavapayBankAccountNumberChanged(
                                                    value,
                                                ),
                                            )
                                        },
                                    )
                                    .size(14)
                                    .padding(10),
                                )
                                .width(Length::Fixed(150.0)),
                        )
                        .push(Space::with_width(Length::Fixed(10.0)))
                        .push(
                            Column::new()
                                .push(text("Account Name").size(14).color(color::GREY_3))
                                .push(Space::with_height(Length::Fixed(5.0)))
                                .push(
                                    form::Form::new_trimmed(
                                        "John Doe",
                                        &state.mavapay_bank_account_name,
                                        |value| {
                                            ViewMessage::BuySell(
                                                BuySellMessage::MavapayBankAccountNameChanged(
                                                    value,
                                                ),
                                            )
                                        },
                                    )
                                    .size(14)
                                    .padding(10),
                                )
                                .width(Length::Fixed(150.0)),
                        )
                        .spacing(10),
                )
                .push(Space::with_height(Length::Fixed(10.0)))
                .push(
                    Row::new()
                        .push(
                            Column::new()
                                .push(text("Bank Code").size(14).color(color::GREY_3))
                                .push(Space::with_height(Length::Fixed(5.0)))
                                .push(
                                    form::Form::new_trimmed(
                                        "011",
                                        &state.mavapay_bank_code,
                                        |value| {
                                            ViewMessage::BuySell(
                                                BuySellMessage::MavapayBankCodeChanged(value),
                                            )
                                        },
                                    )
                                    .size(14)
                                    .padding(10),
                                )
                                .width(Length::Fixed(100.0)),
                        )
                        .push(Space::with_width(Length::Fixed(10.0)))
                        .push(
                            Column::new()
                                .push(text("Bank Name").size(14).color(color::GREY_3))
                                .push(Space::with_height(Length::Fixed(5.0)))
                                .push(
                                    form::Form::new_trimmed(
                                        "First Bank",
                                        &state.mavapay_bank_name,
                                        |value| {
                                            ViewMessage::BuySell(
                                                BuySellMessage::MavapayBankNameChanged(value),
                                            )
                                        },
                                    )
                                    .size(14)
                                    .padding(10),
                                )
                                .width(Length::Fixed(200.0)),
                        )
                        .spacing(10),
                )
                .push(Space::with_height(Length::Fixed(15.0)))
                .push(
                    ui_button::primary(None, "Create Quote")
                        .on_press(ViewMessage::BuySell(BuySellMessage::MavapayCreateQuote))
                        .width(Length::Fill),
                )
                .spacing(5),
        )
        .padding(20)
        .style(theme::card::simple);

        column = column
            .push(exchange_form)
            .push(Space::with_height(Length::Fixed(10.0)))
            .push(text("--- OR ---").size(12).color(color::GREY_3))
            .push(Space::with_height(Length::Fixed(10.0)))
            .push(
                ui_button::primary(None, "Open Secure Checkout")
                    .on_press(ViewMessage::BuySell(BuySellMessage::MavapayOpenPaymentLink))
                    .width(Length::Fill),
            );

        // Quote display with payment confirmation
        if let Some(quote) = &state.mavapay_current_quote {
            let mut quote_column = Column::new()
                .push(ui_text::h5_medium("Quote Created Successfully").color(color::GREEN))
                .push(Space::with_height(Length::Fixed(10.0)))
                .push(
                    Row::new()
                        .push(text("Amount: ").size(14).color(color::GREY_3))
                        .push(
                            text(format!("{} sats", quote.total_amount_in_source_currency))
                                .size(14)
                                .color(color::WHITE),
                        ),
                )
                .push(
                    Row::new()
                        .push(text("Rate: ").size(14).color(color::GREY_3))
                        .push(
                            text(format!("{:.2}", quote.exchange_rate))
                                .size(14)
                                .color(color::WHITE),
                        ),
                )
                .push(
                    Row::new()
                        .push(text("Expires: ").size(14).color(color::GREY_3))
                        .push(text(&quote.expiry).size(14).color(color::ORANGE)),
                );

            // Show payment details if available
            if let Some(payment_details) = &quote.invoice {
                quote_column = quote_column
                    .push(Space::with_height(Length::Fixed(10.0)))
                    .push(text("Lightning Invoice:").size(14).color(color::GREY_3))
                    .push(
                        Container::new(text(payment_details).size(12).color(color::WHITE))
                            .padding(10)
                            .style(theme::card::simple),
                    );
            }

            quote_column = quote_column
                .push(Space::with_height(Length::Fixed(15.0)))
                .push(
                    ui_button::primary(None, "Confirm Payment")
                        .on_press(ViewMessage::BuySell(BuySellMessage::MavapayConfirmPayment(
                            quote.id.clone(),
                        )))
                        .width(Length::Fill),
                );

            let quote_display = Container::new(quote_column.spacing(5))
                .padding(20)
                .style(theme::card::simple);

            column = column
                .push(Space::with_height(Length::Fixed(15.0)))
                .push(quote_display);
        }

        // Payment status display
        if let Some(payment_status) = &state.mavapay_payment_status {
            let status_color = match payment_status.status.as_str() {
                "PAID" | "SUCCESS" => color::GREEN,
                "FAILED" => color::RED,
                "PENDING" => color::ORANGE,
                _ => color::GREY_3,
            };

            let mut status_column = Column::new()
                .push(
                    Row::new()
                        .push(text("💳").size(20))
                        .push(Space::with_width(Length::Fixed(10.0)))
                        .push(ui_text::h5_medium("Payment Status").color(color::WHITE))
                        .align_y(Alignment::Center),
                )
                .push(Space::with_height(Length::Fixed(10.0)))
                .push(
                    Row::new()
                        .push(text("Status: ").size(14).color(color::GREY_3))
                        .push(text(&payment_status.status).size(14).color(status_color)),
                )
                .push(
                    Row::new()
                        .push(text("Amount: ").size(14).color(color::GREY_3))
                        .push(
                            text(format!(
                                "{} {}",
                                payment_status.amount, payment_status.currency
                            ))
                            .size(14)
                            .color(color::WHITE),
                        ),
                );

            if state.mavapay_polling_active {
                status_column = status_column
                    .push(Space::with_height(Length::Fixed(10.0)))
                    .push(
                        Row::new()
                            .push(text("🔄").size(16))
                            .push(Space::with_width(Length::Fixed(5.0)))
                            .push(text("Monitoring payment...").size(14).color(color::ORANGE))
                            .align_y(Alignment::Center),
                    )
                    .push(Space::with_height(Length::Fixed(10.0)))
                    .push(
                        ui_button::secondary(None, "Stop Monitoring")
                            .on_press(ViewMessage::BuySell(BuySellMessage::MavapayStopPolling))
                            .width(Length::Fill),
                    );
            } else if payment_status.status == "PENDING" {
                status_column = status_column
                    .push(Space::with_height(Length::Fixed(10.0)))
                    .push(
                        ui_button::primary(None, "Check Status")
                            .on_press(ViewMessage::BuySell(
                                BuySellMessage::MavapayCheckPaymentStatus(
                                    payment_status.quote_id.clone(),
                                ),
                            ))
                            .width(Length::Fill),
                    );
            }

            let status_display = Container::new(status_column.spacing(5))
                .padding(20)
                .style(theme::card::simple);

            column = column
                .push(Space::with_height(Length::Fixed(15.0)))
                .push(status_display);
        }

        column.spacing(10)
    }
}

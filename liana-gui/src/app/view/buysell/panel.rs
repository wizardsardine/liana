use iced::{
    widget::{container, pick_list, radio, Space},
    Alignment, Length, Task,
};
use iced_webview::{advanced::WebView, Ultralight};

use liana::miniscript::bitcoin::{self, Network};
use liana_ui::{
    color,
    component::{button as ui_button, form, text::text},
    icon::*,
    theme,
    widget::*,
};

use super::flow_state::{
    BuySellFlowState, MavapayFlowMode, MavapayFlowState, MavapayPaymentMethod, NativePage,
};
use crate::app::{
    self,
    view::{BuySellMessage, Message as ViewMessage},
};
use crate::services::mavapay::api::Currency;

#[derive(Debug, Clone, Copy)]
pub enum BuyOrSell {
    Buy,
    Sell,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LabelledAddress {
    pub address: bitcoin::Address,
    pub index: bitcoin::bip32::ChildNumber,
    pub label: Option<String>,
}

impl std::fmt::Display for LabelledAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.label {
            Some(l) => write!(f, "{}: {}", l, self.address),
            None => std::fmt::Display::fmt(&self.address, f),
        }
    }
}

pub struct BuySellPanel {
    // Runtime state - determines which flow is active
    pub flow_state: BuySellFlowState,
    pub modal: app::state::receive::Modal,
    pub buy_or_sell: Option<BuyOrSell>,

    // Common fields (always present)
    pub error: Option<String>,
    pub network: Network,

    // for address generation
    pub wallet: std::sync::Arc<crate::app::wallet::Wallet>,
    pub data_dir: crate::dir::LianaDirectory,
    pub generated_address: Option<LabelledAddress>,

    // Geolocation detection state
    pub detected_country_name: Option<String>,
    pub detected_country_iso: Option<String>,

    // Webview (used by international flow)
    pub webview: Option<WebView<Ultralight, crate::app::state::buysell::WebviewMessage>>,
    pub session_url: Option<String>,
    pub active_page: Option<iced_webview::ViewId>,
}

impl BuySellPanel {
    pub fn new(
        network: bitcoin::Network,
        wallet: std::sync::Arc<crate::app::wallet::Wallet>,
        data_dir: crate::dir::LianaDirectory,
    ) -> Self {
        Self {
            buy_or_sell: None,
            error: None,
            network,
            wallet,
            data_dir,
            generated_address: None,
            modal: app::state::receive::Modal::None,
            detected_country_name: None,
            detected_country_iso: None,
            webview: None,
            session_url: None,
            active_page: None,
            // Start in detecting location state
            flow_state: BuySellFlowState::DetectingLocation,
        }
    }

    /// Opens Onramper widget session (only called for non-Mavapay countries)
    /// Mavapay flow is now handled by SetBuyOrSell message handler
    pub fn start_session(&mut self) -> iced::Task<BuySellMessage> {
        use crate::app::buysell::onramper;

        let Some(iso_code) = self.detected_country_iso.as_ref() else {
            return Task::none();
        };

        // This method is now only called for Onramper (non-Mavapay) flow
        let currency = crate::services::fiat::currency_for_country(iso_code).to_string();

        // prepare parameters
        let address = self
            .generated_address
            .as_ref()
            .map(|a| a.address.to_string());

        let mode = match self.buy_or_sell {
            None => return Task::none(),
            Some(BuyOrSell::Buy) => "buy",
            Some(BuyOrSell::Sell) => "sell",
        };

        match onramper::create_widget_url(&currency, address.as_deref(), &mode) {
            Ok(url) => {
                tracing::info!("🌍 [ONRAMPER] Widget URL created successfully: {url}");
                Task::batch([
                    Task::done(BuySellMessage::WebviewOpenUrl(url)),
                    Task::done(BuySellMessage::SetFlowState(BuySellFlowState::Onramper)),
                ])
            }
            Err(error) => {
                tracing::error!("🌍 [ONRAMPER] Error: {}", error);
                Task::done(BuySellMessage::SessionError(error))
            }
        }
    }

    pub fn set_error(&mut self, error: String) {
        self.error = Some(error);
    }

    pub fn set_login_username(&mut self, v: String) {
        if let BuySellFlowState::Mavapay(ref mut state) = self.flow_state {
            state.login_username.value = v;
            state.login_username.valid = !state.login_username.value.is_empty();
        }
    }

    pub fn set_login_password(&mut self, v: String) {
        if let BuySellFlowState::Mavapay(ref mut state) = self.flow_state {
            state.login_password.value = v;
            state.login_password.valid = !state.login_password.value.is_empty();
        }
    }

    pub fn is_login_form_valid(&self) -> bool {
        if let BuySellFlowState::Mavapay(ref state) = self.flow_state {
            state.login_username.valid && state.login_password.valid
        } else {
            false
        }
    }

    pub fn view<'a>(&'a self) -> iced::Element<'a, ViewMessage, liana_ui::theme::Theme> {
        let webview_active = self.active_page.is_some();
        let is_onramper_active =
            webview_active && matches!(self.flow_state, BuySellFlowState::Onramper);

        let column = {
            let column = Column::new()
                // COINCUBE branding
                .push(
                    Row::new()
                        .push(
                            Container::new(liana_ui::icon::bitcoin_icon().size(24))
                                .style(theme::container::border)
                                .padding(10),
                        )
                        .push(Space::with_width(Length::Fixed(10.0)))
                        .push(
                            Column::new()
                                .push(text("COINCUBE").size(16).color(color::ORANGE))
                                .push(text("BUY/SELL").size(14).color(color::GREY_3))
                                .spacing(2),
                        )
                        .push_maybe({
                            webview_active.then(|| Space::with_width(Length::Fixed(25.0)))
                        })
                        .push_maybe({
                            webview_active.then(|| {
                                ui_button::secondary(Some(reload_icon()), "Reset Widget")
                                    .on_press(ViewMessage::BuySell(BuySellMessage::ResetWidget))
                            })
                        })
                        .align_y(Alignment::Center),
                )
                // Network display banner for Onramper flow
                .push_maybe(is_onramper_active.then(|| {
                    let network_name = match self.network {
                        liana::miniscript::bitcoin::Network::Bitcoin => "Bitcoin Mainnet",
                        liana::miniscript::bitcoin::Network::Testnet => "Bitcoin Testnet",
                        liana::miniscript::bitcoin::Network::Testnet4 => "Bitcoin Testnet4",
                        liana::miniscript::bitcoin::Network::Signet => "Bitcoin Signet",
                        liana::miniscript::bitcoin::Network::Regtest => "Bitcoin Regtest",
                    };
                    let network_color = match self.network {
                        liana::miniscript::bitcoin::Network::Bitcoin => color::GREEN,
                        liana::miniscript::bitcoin::Network::Testnet
                        | liana::miniscript::bitcoin::Network::Testnet4
                        | liana::miniscript::bitcoin::Network::Signet
                        | liana::miniscript::bitcoin::Network::Regtest => color::ORANGE,
                    };

                    Container::new(
                        Row::new()
                            .push(text("Network: ").size(12).color(color::GREY_3))
                            .push(text(network_name).size(12).color(network_color))
                            .spacing(5)
                            .align_y(Alignment::Center),
                    )
                    .padding(8)
                    .style(theme::card::simple)
                }))
                // error display
                .push_maybe(self.error.as_ref().map(|err| {
                    Container::new(text(err).size(14).color(color::RED))
                        .padding(10)
                        .style(theme::card::invalid)
                }))
                .push_maybe(
                    self.error
                        .is_some()
                        .then(|| Space::with_height(Length::Fixed(20.0))),
                )
                .push_maybe((!webview_active).then(|| self.form_view()));

            // attempt to render webview (if available)
            let view = self
                .active_page
                .as_ref()
                .map(|v| {
                    let view = self.webview.as_ref().map(|s| {
                        s.view(*v)
                            .map(|a| ViewMessage::BuySell(BuySellMessage::WebviewAction(a)))
                    });

                    view
                })
                .flatten();

            let column = column.push({
                match view {
                    Some(v) => container(v)
                        .width(Length::Fixed(600.0))
                        .height(Length::Fixed(600.0)),
                    None => container(" ").height(Length::Fixed(150.0)),
                }
            });

            column
                .align_x(Alignment::Center)
                .spacing(7) // Reduced spacing for more compact layout
                .width(Length::Fill)
        };

        Container::new(column)
            .width(Length::Fill)
            .align_y(Alignment::Start)
            .align_x(Alignment::Center)
            .into()
    }

    fn form_view<'a>(&'a self) -> Column<'a, ViewMessage> {
        match &self.flow_state {
            BuySellFlowState::DetectingLocation => self.render_detecting_location(),
            BuySellFlowState::Initialization => self.initialization_flow(),
            BuySellFlowState::Mavapay(state) => self.render_africa_flow(state),
            BuySellFlowState::Onramper => self.render_loading_onramper(),
        }
    }
    fn initialization_flow<'a>(&'a self) -> Column<'a, ViewMessage> {
        use iced::widget::scrollable;
        use liana_ui::component::{
            button, card,
            text::{p2_regular, Text},
        };

        let mut column = Column::new();
        column = match self.generated_address.as_ref() {
            Some(addr) => column
                .push(text("Generated Address").size(14).color(color::GREY_3))
                .push({
                    let address_text = addr.to_string();

                    card::simple(
                        Column::new()
                            .push(
                                Container::new(
                                    scrollable(
                                        Column::new()
                                            .push(Space::with_height(Length::Fixed(10.0)))
                                            .push(
                                                p2_regular(&address_text)
                                                    .small()
                                                    .style(theme::text::secondary),
                                            )
                                            // Space between the address and the scrollbar
                                            .push(Space::with_height(Length::Fixed(10.0))),
                                    )
                                    .direction(
                                        scrollable::Direction::Horizontal(
                                            scrollable::Scrollbar::new().width(2).scroller_width(2),
                                        ),
                                    ),
                                )
                                .width(Length::Fill),
                            )
                            .push(
                                Row::new()
                                    .push(
                                        button::secondary(None, "Verify on hardware device")
                                            .on_press(ViewMessage::Select(0)),
                                    )
                                    .push(Space::with_width(Length::Fill))
                                    .push(
                                        Button::new(qr_code_icon().style(theme::text::secondary))
                                            .on_press(ViewMessage::ShowQrCode(0))
                                            .style(theme::button::transparent_border),
                                    )
                                    .push(
                                        Button::new(clipboard_icon().style(theme::text::secondary))
                                            .on_press(ViewMessage::Clipboard(address_text))
                                            .style(theme::button::transparent_border),
                                    )
                                    .align_y(Alignment::Center),
                            )
                            .spacing(10),
                    )
                    .width(Length::Fill)
                })
                .push(
                    ui_button::primary(Some(globe_icon()), "Start Widget Session")
                        .on_press_maybe(
                            self.detected_country_iso
                                .is_some()
                                .then_some(ViewMessage::BuySell(BuySellMessage::CreateSession)),
                        )
                        .width(iced::Length::Fill),
                ),
            None => column
                .push({
                    let buy_or_sell = self.buy_or_sell.clone();

                    Column::new()
                        .push(
                            ui_button::secondary(
                                Some(bitcoin_icon()),
                                "Buy Bitcoin using Fiat Currencies",
                            )
                            .on_press(ViewMessage::BuySell(BuySellMessage::SetBuyOrSell(
                                BuyOrSell::Buy,
                            )))
                            .style(move |th, st| match buy_or_sell {
                                Some(BuyOrSell::Buy) => liana_ui::theme::button::primary(th, st),
                                _ => liana_ui::theme::button::secondary(th, st),
                            })
                            .padding(30)
                            .width(iced::Length::Fill),
                        )
                        .push(
                            ui_button::secondary(
                                Some(dollar_icon()),
                                "Sell Bitcoin to a Fiat Currency",
                            )
                            .on_press(ViewMessage::BuySell(BuySellMessage::SetBuyOrSell(
                                BuyOrSell::Sell,
                            )))
                            .style(move |th, st| match buy_or_sell {
                                Some(BuyOrSell::Sell) => liana_ui::theme::button::primary(th, st),
                                _ => liana_ui::theme::button::secondary(th, st),
                            })
                            .padding(30)
                            .width(iced::Length::Fill),
                        )
                        .spacing(15)
                        .padding(5)
                })
                .push_maybe({
                    // Only show intermediate buttons for non-Mavapay countries
                    // For Mavapay countries, the flow immediately transitions to AccountSelect
                    let is_mavapay = self
                        .detected_country_iso
                        .as_ref()
                        .map(|iso| crate::services::fiat::mavapay_supported(iso))
                        .unwrap_or(false);

                    (self.buy_or_sell.is_some() && !is_mavapay).then(|| {
                        container(Space::with_height(1))
                            .style(|_| {
                                iced::widget::container::background(iced::Background::Color(
                                    color::GREY_6,
                                ))
                            })
                            .width(Length::Fill)
                    })
                })
                .push_maybe({
                    // Only show "Generate New Address" for Buy in non-Mavapay countries
                    let is_mavapay = self
                        .detected_country_iso
                        .as_ref()
                        .map(|iso| crate::services::fiat::mavapay_supported(iso))
                        .unwrap_or(false);

                    (matches!(self.buy_or_sell, Some(BuyOrSell::Buy)) && !is_mavapay).then(|| {
                        ui_button::secondary(Some(plus_icon()), "Generate New Address")
                            .on_press_maybe(
                                matches!(self.buy_or_sell, Some(BuyOrSell::Buy)).then_some(
                                    ViewMessage::BuySell(BuySellMessage::CreateNewAddress),
                                ),
                            )
                            .width(iced::Length::Fill)
                    })
                })
                .push_maybe({
                    // Only show "Start Widget Session" for Sell in non-Mavapay countries
                    let is_mavapay = self
                        .detected_country_iso
                        .as_ref()
                        .map(|iso| crate::services::fiat::mavapay_supported(iso))
                        .unwrap_or(false);

                    (matches!(self.buy_or_sell, Some(BuyOrSell::Sell)) && !is_mavapay).then(|| {
                        ui_button::secondary(Some(globe_icon()), "Start Widget Session")
                            .on_press_maybe(
                                self.detected_country_iso
                                    .is_some()
                                    .then_some(ViewMessage::BuySell(BuySellMessage::CreateSession)),
                            )
                            .width(iced::Length::Fill)
                    })
                }),
        };

        column
            .align_x(Alignment::Center)
            .spacing(12)
            .max_width(640)
            .width(Length::Fill)
    }

    fn render_africa_flow<'a>(&'a self, state: &'a MavapayFlowState) -> Column<'a, ViewMessage> {
        match state.native_page {
            NativePage::AccountSelect => self.native_account_select_form(state),
            NativePage::Login => self.native_login_form(state),
            NativePage::Register => self.native_register_form(state),
            NativePage::VerifyEmail => self.native_verify_email_form(state),
            NativePage::CoincubePay => self.coincube_pay_form(state),
        }
    }

    fn render_detecting_location<'a>(&'a self) -> Column<'a, ViewMessage> {
        use liana_ui::component::text as ui_text;

        Column::new()
            .push(Space::with_height(Length::Fixed(30.0)))
            .push(ui_text::p1_bold("Detecting your location...").color(color::WHITE))
            .push(Space::with_height(Length::Fixed(20.0)))
            .push(text("Please wait...").size(14).color(color::GREY_3))
            .align_x(Alignment::Center)
            .spacing(10)
            .max_width(500)
            .width(Length::Fill)
    }

    fn render_loading_onramper<'a>(&'a self) -> Column<'a, ViewMessage> {
        use liana_ui::component::text as ui_text;
        let info = if let Some(country_name) = &self.detected_country_name {
            format!("Country detected: {}. Opening Onramper...", country_name)
        } else {
            "Opening Onramper...".to_string()
        };
        Column::new()
            .push_maybe(self.error.as_ref().map(|err| {
                Container::new(text(err).size(14).color(color::RED))
                    .padding(10)
                    .style(theme::card::simple)
            }))
            .push(Space::with_height(Length::Fixed(30.0)))
            .push(ui_text::p1_bold(&info).color(color::WHITE))
            .push(Space::with_height(Length::Fixed(20.0)))
            .push(text("Loading...").size(14).color(color::GREY_3))
            .align_x(Alignment::Center)
            .spacing(10)
            .max_width(500)
            .width(Length::Fill)
    }
}

impl BuySellPanel {
    fn native_account_select_form<'a>(
        &'a self,
        state: &'a MavapayFlowState,
    ) -> Column<'a, ViewMessage> {
        use liana_ui::component::card as ui_card;
        use liana_ui::component::text as ui_text;
        use liana_ui::icon::{building_icon, person_icon};

        let header = ui_text::h3("Choose your account type").color(color::WHITE);

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
            .push(Space::with_height(Length::Fixed(30.0)))
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

    fn native_login_form<'a>(&'a self, state: &'a MavapayFlowState) -> Column<'a, ViewMessage> {
        use liana_ui::component::form;
        use liana_ui::component::text as ui_text;

        let header = ui_text::h3("Sign in to your account").color(color::WHITE);

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

impl BuySellPanel {
    fn native_register_form<'a>(&'a self, state: &'a MavapayFlowState) -> Column<'a, ViewMessage> {
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
    pub fn is_registration_valid(&self, state: &MavapayFlowState) -> bool {
        let email_ok = state.email.value.contains('@') && state.email.value.contains('.');
        let pw_ok = self.is_password_valid(state) && state.password1.value == state.password2.value;
        !state.first_name.value.is_empty()
            && !state.last_name.value.is_empty()
            && email_ok
            && pw_ok
            && state.terms_accepted
    }

    #[inline]
    pub fn is_password_valid(&self, state: &MavapayFlowState) -> bool {
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

    pub fn get_password_validation_message(&self, state: &MavapayFlowState) -> Option<String> {
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
        if let BuySellFlowState::Mavapay(ref mut state) = self.flow_state {
            state.email_verification_status = verified;
        }
    }

    fn native_verify_email_form<'a>(
        &'a self,
        state: &'a MavapayFlowState,
    ) -> Column<'a, ViewMessage> {
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
                ui_text::p2_regular(format!("Email sent to: {}", state.email.value))
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

    fn coincube_pay_form<'a>(&'a self, state: &'a MavapayFlowState) -> Column<'a, ViewMessage> {
        use liana_ui::component::{button as ui_button, text as ui_text};

        let header = Row::new()
            .push(Space::with_width(Length::Fill))
            .push(ui_text::h4_bold("Bitcoin ↔ Fiat Exchange").color(color::WHITE))
            .push(Space::with_width(Length::Fill))
            .align_y(Alignment::Center);

        let mut column = Column::new()
            .push(header)
            .push(Space::with_height(Length::Fixed(20.0)));

        // Error display
        if let Some(error) = &self.error {
            column = column
                .push(
                    Container::new(text(error).size(14).color(color::RED))
                        .padding(10)
                        .style(theme::card::invalid)
                        .width(Length::Fixed(600.0)), // Match form width
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
                    .style(theme::card::simple)
                    .width(Length::Fixed(600.0)), // Match form width
                )
                .push(Space::with_height(Length::Fixed(15.0)));
        }

        // Exchange form with payment mode radio buttons
        let mut form_column = Column::new()
            .push(Space::with_height(Length::Fixed(15.0)))
            // Payment mode selection with radio buttons
            .push(text("Payment Mode").size(14).color(color::GREY_3))
            .push(Space::with_height(Length::Fixed(8.0)))
            .push(
                Row::new()
                    .push(
                        radio(
                            "Create Quote",
                            MavapayFlowMode::CreateQuote,
                            Some(state.mavapay_flow_mode),
                            |mode| {
                                ViewMessage::BuySell(BuySellMessage::MavapayFlowModeChanged(mode))
                            },
                        )
                        .size(16)
                        .text_size(14),
                    )
                    .push(Space::with_width(Length::Fixed(20.0)))
                    .push(
                        radio(
                            "One-time Payment",
                            MavapayFlowMode::OneTimePayment,
                            Some(state.mavapay_flow_mode),
                            |mode| {
                                ViewMessage::BuySell(BuySellMessage::MavapayFlowModeChanged(mode))
                            },
                        )
                        .size(16)
                        .text_size(14),
                    )
                    .align_y(Alignment::Center),
            )
            .push(Space::with_height(Length::Fixed(15.0)))
            // Amount field (common to both modes)
            .push(text("Amount").size(14).color(color::GREY_3))
            .push(Space::with_height(Length::Fixed(5.0)))
            .push(
                Container::new(
                    form::Form::new_trimmed("100000", &state.mavapay_amount, |value| {
                        ViewMessage::BuySell(BuySellMessage::MavapayAmountChanged(value))
                    })
                    .size(14)
                    .padding(10),
                )
                .width(Length::Fixed(200.0)),
            )
            .push(Space::with_height(Length::Fixed(15.0)));

        // Conditional fields based on mode
        match state.mavapay_flow_mode {
            MavapayFlowMode::CreateQuote => {
                // Show From/To currency dropdowns for quote mode
                form_column = form_column
                    .push(
                        Row::new()
                            .push(
                                Column::new()
                                    .push(text("From").size(14).color(color::GREY_3))
                                    .push(Space::with_height(Length::Fixed(5.0)))
                                    .push(
                                        pick_list(
                                            Currency::all(),
                                            Currency::parse(&state.mavapay_source_currency.value),
                                            |currency| {
                                                ViewMessage::BuySell(
                                                    BuySellMessage::MavapaySourceCurrencyChanged(
                                                        currency.as_str().to_string(),
                                                    ),
                                                )
                                            },
                                        )
                                        .style(theme::pick_list::primary)
                                        .padding(10),
                                    )
                                    .width(Length::Fixed(250.0)),
                            )
                            .push(Space::with_width(Length::Fixed(15.0)))
                            .push(
                                Column::new()
                                    .push(text("To").size(14).color(color::GREY_3))
                                    .push(Space::with_height(Length::Fixed(5.0)))
                                    .push(
                                        pick_list(
                                            Currency::all(),
                                            Currency::parse(&state.mavapay_target_currency.value),
                                            |currency| {
                                                ViewMessage::BuySell(
                                                    BuySellMessage::MavapayTargetCurrencyChanged(
                                                        currency.as_str().to_string(),
                                                    ),
                                                )
                                            },
                                        )
                                        .style(theme::pick_list::primary)
                                        .padding(10),
                                    )
                                    .width(Length::Fixed(250.0)),
                            )
                            .spacing(10),
                    )
                    .push(Space::with_height(Length::Fixed(15.0)));

                // Bank Account Details (only when selling BTC in Create Quote mode)
                if state.mavapay_source_currency.value.as_str() == "BTCSAT" {
                    form_column = form_column
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
                                                        BuySellMessage::MavapayBankAccountNumberChanged(value),
                                                    )
                                                },
                                            )
                                            .size(14)
                                            .padding(10),
                                        )
                                        .width(Length::Fixed(250.0)),
                                )
                                .push(Space::with_width(Length::Fixed(15.0)))
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
                                                        BuySellMessage::MavapayBankAccountNameChanged(value),
                                                    )
                                                },
                                            )
                                            .size(14)
                                            .padding(10),
                                        )
                                        .width(Length::Fixed(250.0)),
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
                                        .width(Length::Fixed(120.0)),
                                )
                                .push(Space::with_width(Length::Fixed(15.0)))
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
                                        .width(Length::Fixed(385.0)),
                                )
                                .spacing(10),
                        )
                        .push(Space::with_height(Length::Fixed(15.0)));
                }
            }
            MavapayFlowMode::OneTimePayment => {
                // Show Settlement Currency and Payment Method for one-time payment
                form_column = form_column
                    .push(text("Settlement Currency").size(14).color(color::GREY_3))
                    .push(Space::with_height(Length::Fixed(5.0)))
                    .push(
                        pick_list(
                            ["BTC", "NGN", "ZAR", "KES"],
                            Some(state.mavapay_settlement_currency.value.as_str()),
                            |currency| {
                                ViewMessage::BuySell(
                                    BuySellMessage::MavapaySettlementCurrencyChanged(
                                        currency.to_string(),
                                    ),
                                )
                            },
                        )
                        .style(theme::pick_list::primary)
                        .padding(10)
                        .width(Length::Fixed(250.0)),
                    )
                    .push(Space::with_height(Length::Fixed(15.0)))
                    .push(text("Payment Method").size(14).color(color::GREY_3))
                    .push(Space::with_height(Length::Fixed(5.0)))
                    .push(
                        pick_list(
                            MavapayPaymentMethod::all(),
                            Some(state.mavapay_payment_method),
                            |method| {
                                ViewMessage::BuySell(BuySellMessage::MavapayPaymentMethodChanged(
                                    method,
                                ))
                            },
                        )
                        .style(theme::pick_list::primary)
                        .padding(10)
                        .width(Length::Fixed(250.0)),
                    )
                    .push(Space::with_height(Length::Fixed(15.0)));
            }
        }

        // Button section
        let button_text = "Payment";

        let button_message = match state.mavapay_flow_mode {
            MavapayFlowMode::CreateQuote => BuySellMessage::MavapayCreateQuote,
            MavapayFlowMode::OneTimePayment => BuySellMessage::MavapayOpenPaymentLink,
        };

        form_column = form_column
            .push(
                ui_button::primary(None, button_text)
                    .on_press(ViewMessage::BuySell(button_message))
                    .width(Length::Fill),
            )
            .spacing(5);

        let exchange_form = Container::new(form_column)
            .padding(20)
            .style(theme::card::simple)
            .width(Length::Fixed(600.0)); // Fixed width for consistent layout

        column = column.push(exchange_form);

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
            if !quote.invoice.is_empty() {
                quote_column = quote_column
                    .push(Space::with_height(Length::Fixed(10.0)))
                    .push(text("Lightning Invoice:").size(14).color(color::GREY_3))
                    .push(
                        Container::new(text(&quote.invoice).size(12).color(color::WHITE))
                            .padding(10)
                            .style(theme::card::simple),
                    );
            }

            // Show NGN bank details if available (for buy-BTC flow)
            if !quote.bank_name.is_empty() {
                quote_column = quote_column
                    .push(Space::with_height(Length::Fixed(10.0)))
                    .push(
                        text("Pay to this bank account:")
                            .size(14)
                            .color(color::GREY_3),
                    )
                    .push(Space::with_height(Length::Fixed(5.0)))
                    .push(
                        Container::new(
                            Column::new()
                                .push(
                                    text(format!("Bank: {}", quote.bank_name))
                                        .size(12)
                                        .color(color::WHITE),
                                )
                                .push(
                                    text(format!(
                                        "Account Number: {}",
                                        quote.ngn_bank_account_number
                                    ))
                                    .size(12)
                                    .color(color::WHITE),
                                )
                                .push(
                                    text(format!("Account Name: {}", quote.ngn_account_name))
                                        .size(12)
                                        .color(color::WHITE),
                                )
                                .spacing(5),
                        )
                        .padding(10)
                        .style(theme::card::simple),
                    );
            }

            // Note: "Confirm Payment" button removed - payment page opens automatically
            // after quote creation in the simplified flow

            let quote_display = Container::new(quote_column.spacing(5))
                .padding(20)
                .style(theme::card::simple)
                .width(Length::Fixed(600.0)); // Match form width

            column = column
                .push(Space::with_height(Length::Fixed(15.0)))
                .push(quote_display);
        }

        column
            .spacing(10)
            .align_x(Alignment::Center)
            .width(Length::Fill)
    }
}

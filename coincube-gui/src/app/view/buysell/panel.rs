use iced::{
    widget::{container, pick_list, text_input, Space},
    Alignment, Length,
};

use coincube_core::miniscript::bitcoin::{self, Network};
use coincube_ui::{
    color,
    component::{
        button, card,
        text::{self, text},
    },
    icon::*,
    theme,
    widget::*,
};

use crate::app::{
    self,
    view::{BuySellMessage, Message as ViewMessage},
};

use crate::services::coincube::*;

#[derive(Debug, Clone)]
pub enum BuyOrSell {
    Sell,
    Buy { address: LabelledAddress },
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

pub enum BuySellFlowState {
    /// Detecting user's location via IP geolocation, true if geolocation failed and the user is manually prompted
    DetectingLocation(bool),
    /// Registers a new user
    Register {
        legal_name: String,
        password1: String,
        password2: String,
        email: String,
    },
    /// User email verification UI
    VerifyEmail {
        email: String,
        password: String,
        checking: bool,
    },
    /// Logs in a user to their existing coincube account
    Login { email: String, password: String },
    /// Flow for resetting a forgotten password
    PasswordReset { email: String, sent: bool },
    /// Renders an interface to either generate a new address for bitcoin deposit, or skip to selling BTC
    Initialization {
        modal: app::state::vault::receive::Modal,
        buy_or_sell_selected: Option<bool>,
        buy_or_sell: Option<BuyOrSell>, // `buy` mode always has an address included for deposit
    },
    /// Nigeria, Kenya and South Africa, ie Mavapay supported countries
    Mavapay(super::mavapay::MavapayState),
    /// A webview is currently active, and is rendered instead of a buysell UI
    WebviewRenderer {
        active: iced_wry::IcedWebview,
        manager: iced_wry::IcedWebviewManager,
    },
}

impl BuySellFlowState {
    pub fn name(&self) -> &'static str {
        match self {
            BuySellFlowState::DetectingLocation(..) => "DetectingLocation",
            BuySellFlowState::Register { .. } => "Register",
            BuySellFlowState::VerifyEmail { .. } => "VerifyEmail",
            BuySellFlowState::Login { .. } => "Login",
            BuySellFlowState::PasswordReset { .. } => "PasswordReset",
            BuySellFlowState::Initialization { .. } => "Initialization",
            BuySellFlowState::Mavapay(..) => "Mavapay",
            BuySellFlowState::WebviewRenderer { .. } => "WebviewRenderer",
        }
    }
}

pub struct BuySellPanel {
    // Runtime state machine - determines which flow is active
    pub step: BuySellFlowState,

    // Common fields (always present)
    // TODO: Display errors using the globally provided facilities instead
    pub error: Option<String>,
    pub network: Network,

    // services used by several buysell providers
    pub coincube_client: crate::services::coincube::CoincubeClient,
    pub detected_country: Option<crate::services::coincube::Country>,

    // coincube session information, restored from OS keyring
    pub login: Option<LoginResponse>,

    // only really useful for address generation
    pub wallet: std::sync::Arc<crate::app::wallet::Wallet>,
}

impl BuySellPanel {
    pub fn new(
        network: bitcoin::Network,
        wallet: std::sync::Arc<crate::app::wallet::Wallet>,
    ) -> Self {
        BuySellPanel {
            // Start in detecting location state
            step: BuySellFlowState::DetectingLocation(false),
            error: None,
            wallet,
            network,
            // API state
            coincube_client: crate::services::coincube::CoincubeClient::new(),
            detected_country: None,
            login: None,
        }
    }

    pub fn view<'a>(&'a self) -> iced::Element<'a, ViewMessage, theme::Theme> {
        let column = {
            let column = Column::new()
                .push(Space::with_height(60))
                // COINCUBE branding
                .push(
                    Row::new()
                        .push(
                            Row::new()
                                .push(text::h4_bold("COIN").color(color::ORANGE))
                                .push(text::h4_bold("CUBE").color(color::WHITE))
                                .spacing(0),
                        )
                        .push(Space::with_width(Length::Fixed(8.0)))
                        .push(text::h5_regular("BUY/SELL").color(color::GREY_3))
                        // TODO: Render a small `Start Over` button for resetting the panel state
                        .align_y(Alignment::Center),
                )
                // error display
                .push_maybe(self.error.as_ref().map(|err| {
                    Container::new(text(err).size(12).style(theme::text::error).center())
                        .padding(10)
                        .style(theme::card::invalid)
                }))
                .push_maybe(
                    self.error
                        .is_some()
                        .then(|| Space::with_height(Length::Fixed(20.0))),
                )
                // render flow state
                .push({
                    match &self.step {
                        // user management
                        BuySellFlowState::Login { .. } => self.login_ux(),
                        BuySellFlowState::Register { .. } => self.registration_ux(),
                        BuySellFlowState::PasswordReset { .. } => self.password_reset_ux(),
                        BuySellFlowState::VerifyEmail { .. } => self.email_verification_ux(),

                        BuySellFlowState::DetectingLocation(..) => self.geolocation_ux(),
                        BuySellFlowState::Initialization { .. } => self.initialization_ux(),
                        BuySellFlowState::WebviewRenderer { .. } => self.webview_ux(),

                        BuySellFlowState::Mavapay(state) => super::mavapay::ui::form(state),
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
}

// TODO: Use labels instead of placeholders for all input forms
impl BuySellPanel {
    fn login_ux<'a>(self: &'a BuySellPanel) -> iced::Element<'a, ViewMessage, theme::Theme> {
        let BuySellFlowState::Login { email, password } = &self.step else {
            unreachable!();
        };

        let col =
            iced::widget::column![
                // header
                text::h3("Sign in to your account").color(color::WHITE),
                Space::with_height(Length::Fixed(35.0)),
                // input fields
                text_input("Email", email)
                    .on_input(|e| BuySellMessage::LoginUsernameChanged(e))
                    .size(16)
                    .padding(15),
                Space::with_height(Length::Fixed(5.0)),
                text_input("Password", password)
                    .secure(true)
                    .on_input(|p| BuySellMessage::LoginPasswordChanged(p))
                    .on_submit_maybe(
                        (email.contains('.') && email.contains('@') && !password.is_empty())
                            .then_some(BuySellMessage::SubmitLogin {
                                skip_email_verification: false
                            }),
                    )
                    .size(16)
                    .padding(15),
                Space::with_height(Length::Fixed(15.0)),
                // submit button
                button::primary(None, "Log In")
                    .on_press_maybe(
                        (email.contains('.') && email.contains('@') && !password.is_empty())
                            .then_some(BuySellMessage::SubmitLogin {
                                skip_email_verification: false
                            }),
                    )
                    .width(Length::Fill),
                Space::with_height(Length::Fixed(10.0)),
                // separator
                container(Space::new(iced::Length::Fill, iced::Length::Fixed(3.0)))
                    .style(|_| { color::GREY_6.into() }),
                Space::with_height(Length::Fixed(5.0)),
                // sign-up redirect
                iced::widget::button(
                    text::p2_regular("Don't have an account? Sign up").color(color::BLUE),
                )
                .style(theme::button::link)
                .on_press(BuySellMessage::CreateNewAccount),
                // password reset button
                iced::widget::button(
                    text::p2_regular("Forgot your Password? Reset it here...").color(color::ORANGE),
                )
                .style(theme::button::link)
                .on_press(BuySellMessage::ResetPassword)
            ]
            .align_x(Alignment::Center)
            .spacing(2)
            .max_width(500)
            .width(Length::Fill);

        let elem: iced::Element<BuySellMessage, theme::Theme> = col.into();
        elem.map(|b| ViewMessage::BuySell(b))
    }

    fn password_reset_ux<'a>(
        self: &'a BuySellPanel,
    ) -> iced::Element<'a, ViewMessage, theme::Theme> {
        let BuySellFlowState::PasswordReset { email, sent } = &self.step else {
            unreachable!()
        };

        let col = iced::widget::column![
            Space::with_height(Length::Fixed(15.0)),
            text::p1_bold("Password Reset Form"),
            Space::with_height(Length::Fixed(10.0)),
            iced::widget::row![
                container(email_icon().color(color::BLACK).size(20))
                    .style(|_| {
                        iced::widget::container::Style::default()
                            .border(iced::Border {
                                color: color::GREY_1,
                                width: 0.5,
                                radius: 1.0.into(),
                            })
                            .background(color::GREY_1)
                    })
                    .padding(8.0),
                container({
                    let el: iced::Element<BuySellMessage, theme::Theme> = match sent {
                        true => container(
                            text(email)
                                .style(theme::text::success)
                                .size(20)
                                .center()
                                .width(Length::Fill),
                        )
                        .padding(8)
                        .into(),
                        false => text_input("Your e-mail here: ", email)
                            .width(Length::Fill)
                            .size(20)
                            .padding(8)
                            .style(|th, st| {
                                let mut style = theme::text_input::primary(th, st);
                                style.border.radius = 0.into();
                                style.border.width = 0.0;
                                style
                            })
                            .on_input(|s| BuySellMessage::EmailChanged(s))
                            .on_submit(BuySellMessage::SendPasswordResetEmail)
                            .into(),
                    };

                    el
                })
                .style(|_| iced::widget::container::Style::default().border(
                    iced::Border {
                        color: color::GREY_1,
                        width: 0.5,
                        radius: 1.0.into()
                    }
                ))
            ]
            .height(40.0)
        ]
        .push(
            iced::widget::column![
                Space::with_height(Length::Fixed(10.0)),
                // separator
                container(Space::new(iced::Length::Fill, iced::Length::Fixed(2.0)))
                    .style(|_| { color::GREY_7.into() }),
                Space::with_height(Length::Fixed(10.0)),
                match sent {
                    // log-in redirect
                    true => iced::widget::button(
                        text::p2_regular("Recovery Email Sent! Return to Log-In")
                            .color(color::BLUE),
                    )
                    .style(theme::button::link)
                    .on_press(BuySellMessage::ReturnToLogin),
                    // sends the password reset email
                    false =>
                        iced::widget::button(text::p2_medium("Proceed").size(16).width(80).center())
                            .on_press_maybe(
                                (!*sent && email.contains('.') && email.contains('@'))
                                    .then_some(BuySellMessage::SendPasswordResetEmail)
                            ),
                },
            ]
            .align_x(iced::Alignment::Center),
        )
        .align_x(iced::Alignment::Center)
        .spacing(2)
        .max_width(400)
        .width(Length::Fill);

        let elem: iced::Element<BuySellMessage, theme::Theme> = col.into();
        elem.map(|b| ViewMessage::BuySell(b))
    }

    fn registration_ux<'a>(self: &'a BuySellPanel) -> iced::Element<'a, ViewMessage, theme::Theme> {
        let BuySellFlowState::Register {
            legal_name,
            password1,
            password2,
            email,
        } = &self.step
        else {
            unreachable!();
        };

        // TODO: include form validation messages
        let col = iced::widget::column![
            // Top bar with previous
            Button::new(
                Row::new()
                    .push(previous_icon().color(color::GREY_2))
                    .push(Space::with_width(Length::Fixed(5.0)))
                    .push(text::p1_medium("Previous").color(color::GREY_2))
                    .spacing(5)
                    .align_y(Alignment::Center),
            )
            .style(|_, _| iced::widget::button::Style {
                background: None,
                text_color: color::GREY_2,
                border: iced::Border::default(),
                shadow: iced::Shadow::default(),
            })
            .on_press(BuySellMessage::ResetWidget),
            Space::with_height(Length::Fixed(10.0)),
            // Title and subtitle
            iced::widget::column![
                text::h3("Create an Account").color(color::WHITE),
                text::p2_regular("Get started with your personal Bitcoin wallet. Buy, store, and manage crypto securely, all in one place.").color(color::GREY_3)
            ]
            .spacing(10)
            .align_x(Alignment::Center),
            Space::with_height(Length::Fixed(20.0)),
            // Name Input
            text_input("Full Legal Name: ", legal_name).on_input(|v| BuySellMessage::LegalNameChanged(v))
                .width(Length::Fill)
                .size(16)
                .padding(15),
            // Email Input
            text_input("Email Address", email).on_input(|v| {
                BuySellMessage::EmailChanged(v)
            })
            .size(16)
            .padding(15),
            Space::with_height(Length::Fixed(10.0)),
            // Password Inputs
            text_input("Password", password1).on_input(|v| {
                BuySellMessage::Password1Changed(v)
            })
            .size(16)
            .padding(15)
            .secure(true),
            text_input("Confirm Password", password2).on_input(|v| {
                BuySellMessage::Password2Changed(v)
            })
            .on_submit_maybe(
                (!legal_name.is_empty() && email.contains('.') &&  email.contains('@')  && !password1.is_empty() && (password1 == password2))
                    .then_some(BuySellMessage::SubmitRegistration),
            )
            .size(16)
            .padding(15)
            .secure(true),
            Space::with_height(Length::Fixed(20.0)),
            button::primary(None, "Create Account")
                .on_press_maybe(
                    (!legal_name.is_empty() && email.contains('.') &&  email.contains('@')  && !password1.is_empty() && (password1 == password2))
                        .then_some(BuySellMessage::SubmitRegistration),
                )
                .width(Length::Fill),
        ]
        .align_x(Alignment::Center)
        .spacing(5)
        .max_width(500)
        .width(Length::Fill);

        let elem: iced::Element<BuySellMessage, theme::Theme> = col.into();
        elem.map(|b| ViewMessage::BuySell(b))
    }

    fn email_verification_ux<'a>(
        self: &'a BuySellPanel,
    ) -> iced::Element<'a, ViewMessage, theme::Theme> {
        let BuySellFlowState::VerifyEmail {
            email, checking, ..
        } = &self.step
        else {
            unreachable!()
        };

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
                .on_press(BuySellMessage::ResetWidget),
            )
            .align_y(Alignment::Center);

        // Widget title
        let title = match checking {
            true => text::p2_regular("Verification email has been sent, it's your turn now")
                .color(color::GREY_3),
            false => text::p2_regular("We need to verify your email before you continue")
                .color(color::WHITE),
        };

        // Email display
        let email_display = Column::new()
            .push(text::p2_regular(email).color(color::WHITE))
            .spacing(10)
            .align_x(Alignment::Center);

        // Action buttons
        let action_buttons = match checking {
            true => {
                // TODO: Add some animation, probably make this UI nicer
                Row::new()
                    .push(
                        text::p1_italic(
                            "You'll be automatically logged in after verifying your email",
                        )
                        .width(Length::Fill)
                        .center(),
                    )
                    .spacing(10)
            }
            false => Row::new()
                .push(
                    button::secondary(Some(reload_icon()), "Check Status")
                        .on_press(BuySellMessage::CheckEmailVerificationStatus)
                        .width(Length::FillPortion(1)),
                )
                .push(Space::with_width(Length::Fixed(10.0)))
                .push(
                    button::primary(Some(email_icon()), "Resend Email")
                        .on_press(BuySellMessage::SendVerificationEmail),
                )
                .spacing(10)
                .align_y(Alignment::Center),
        };

        let col = iced::widget::column![
            top_bar,
            Space::with_height(Length::Fixed(10.0)),
            title,
            Space::with_height(Length::Fixed(30.0)),
            email_display,
            Space::with_height(Length::Fixed(30.0)),
            action_buttons,
        ]
        .align_x(Alignment::Center)
        .spacing(5)
        .max_width(500)
        .width(Length::Fill);

        let elem: iced::Element<BuySellMessage, theme::Theme> = col.into();
        elem.map(|b| ViewMessage::BuySell(b))
    }

    fn webview_ux<'a>(self: &'a BuySellPanel) -> iced::Element<'a, ViewMessage, theme::Theme> {
        let BuySellFlowState::WebviewRenderer { active, .. } = &self.step else {
            unreachable!()
        };

        let col = iced::widget::column![
            active.view(Length::Fixed(640.0), Length::Fixed(600.0)),
            // Network display banner
            Space::with_height(Length::Fixed(15.0)),
            {
                let (network_name, network_color) = match self.network {
                    coincube_core::miniscript::bitcoin::Network::Bitcoin => {
                        ("Bitcoin Mainnet", color::GREEN)
                    }
                    coincube_core::miniscript::bitcoin::Network::Testnet => {
                        ("Bitcoin Testnet", color::ORANGE)
                    }
                    coincube_core::miniscript::bitcoin::Network::Testnet4 => {
                        ("Bitcoin Testnet4", color::ORANGE)
                    }
                    coincube_core::miniscript::bitcoin::Network::Signet => {
                        ("Bitcoin Signet", color::BLUE)
                    }
                    coincube_core::miniscript::bitcoin::Network::Regtest => {
                        ("Bitcoin Regtest", color::RED)
                    }
                };

                iced::widget::row![
                    // currently selected bitcoin network display
                    text("Network: ").size(12).color(color::GREY_3),
                    text(network_name).size(12).color(network_color),
                    // render a button that closes the webview
                    Space::with_width(Length::Fixed(20.0)),
                    {
                        button::secondary(Some(arrow_back()), "Start Over")
                            .on_press(BuySellMessage::ResetWidget)
                            .width(iced::Length::Fixed(300.0))
                    }
                ]
                .spacing(5)
                .align_y(Alignment::Center)
            }
        ];

        let elem: iced::Element<BuySellMessage, theme::Theme> = col.into();
        elem.map(|b| ViewMessage::BuySell(b))
    }

    fn initialization_ux<'a>(&'a self) -> iced::Element<'a, ViewMessage, theme::Theme> {
        use iced::widget::scrollable;

        let BuySellFlowState::Initialization {
            buy_or_sell_selected,
            buy_or_sell,
            ..
        } = &self.step
        else {
            unreachable!()
        };

        let mut column = Column::new();
        column = match buy_or_sell {
            Some(BuyOrSell::Buy { address }) => column
                .push(
                    text::p1_italic("Bitcoin will be deposited in the following address")
                        .color(color::GREY_2),
                )
                .push({
                    let address_text = address.to_string();

                    card::simple(
                        Column::new()
                            .push(
                                Container::new(
                                    scrollable(
                                        Column::new()
                                            .push(Space::with_height(Length::Fixed(10.0)))
                                            .push(
                                                text::Text::small(text::p2_regular(&address_text))
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
                    button::primary(Some(globe_icon()), "Continue")
                        .on_press_maybe(
                            self.detected_country
                                .is_some()
                                .then_some(ViewMessage::BuySell(BuySellMessage::StartSession)),
                        )
                        .width(iced::Length::Fill),
                ),
            _ => column
                .push({
                    Column::new()
                        .push(
                            button::secondary(
                                Some(bitcoin_icon()),
                                "Buy Bitcoin using Fiat Currencies",
                            )
                            .on_press(ViewMessage::BuySell(BuySellMessage::SelectBuyOrSell(true)))
                            .style({
                                match buy_or_sell_selected {
                                    Some(true) => coincube_ui::theme::button::primary,
                                    _ => coincube_ui::theme::button::secondary,
                                }
                            })
                            .padding(30)
                            .width(iced::Length::Fill),
                        )
                        .push(
                            button::secondary(
                                Some(dollar_icon()),
                                "Sell Bitcoin to a Fiat Currency",
                            )
                            .on_press(ViewMessage::BuySell(BuySellMessage::SelectBuyOrSell(false)))
                            .style({
                                match buy_or_sell_selected {
                                    Some(false) => coincube_ui::theme::button::primary,
                                    _ => coincube_ui::theme::button::secondary,
                                }
                            })
                            .padding(30)
                            .width(iced::Length::Fill),
                        )
                        .spacing(15)
                        .padding(5)
                })
                .push(
                    iced::widget::container(Space::with_height(3))
                        .style(|_| {
                            iced::widget::container::background(iced::Background::Color(
                                color::GREY_3,
                            ))
                        })
                        .width(Length::Fill),
                )
                .push_maybe({
                    (matches!(buy_or_sell_selected, Some(true))).then(|| {
                        button::secondary(Some(plus_icon()), "Generate New Address")
                            .on_press(ViewMessage::BuySell(BuySellMessage::CreateNewAddress))
                            .width(iced::Length::Fill)
                    })
                })
                .push_maybe({
                    (matches!(buy_or_sell_selected, Some(false))).then(|| {
                        button::secondary(Some(globe_icon()), "Continue")
                            .on_press_maybe(
                                self.detected_country
                                    .is_some()
                                    .then_some(ViewMessage::BuySell(BuySellMessage::StartSession)),
                            )
                            .width(iced::Length::Fill)
                    })
                })
                .push_maybe({
                    buy_or_sell_selected.is_none().then(|| {
                        button::secondary(Some(escape_icon()), "Log Out")
                            .on_press(ViewMessage::BuySell(BuySellMessage::LogOut))
                            .width(iced::Length::Fill)
                    })
                }),
        }
        .align_x(Alignment::Center)
        .spacing(12)
        .max_width(640)
        .width(Length::Fill);

        column.into()
    }

    fn geolocation_ux<'a>(&'a self) -> iced::Element<'a, ViewMessage, theme::Theme> {
        let BuySellFlowState::DetectingLocation(manual) = &self.step else {
            unreachable!()
        };

        let col = match *manual {
            true => Column::new()
                .push(
                    pick_list(
                        crate::services::coincube::get_countries(),
                        self.detected_country.as_ref(),
                        |c| BuySellMessage::CountryDetected(Ok(c)),
                    )
                    .padding(10)
                    .placeholder("Select Country: "),
                )
                .align_x(Alignment::Center)
                .width(Length::Fill),
            false => Column::new()
                .push(Space::with_height(Length::Fixed(30.0)))
                .push(text::p1_bold("Detecting your location...").color(color::WHITE))
                .push(Space::with_height(Length::Fixed(20.0)))
                .push(text("Please wait...").size(14).color(color::GREY_3))
                .align_x(Alignment::Center)
                .spacing(10)
                .max_width(500)
                .width(Length::Fill),
        };

        let elem: iced::Element<BuySellMessage, theme::Theme> = col.into();
        elem.map(|b| ViewMessage::BuySell(b))
    }
}

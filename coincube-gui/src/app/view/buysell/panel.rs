use iced::{
    widget::{container, pick_list, text_input, Space},
    Alignment, Length,
};

use coincube_core::miniscript::bitcoin;
use coincube_ui::{
    color,
    component::{button, text},
    icon::*,
    theme,
    widget::*,
};

use crate::app::view::{BuySellMessage, Message as ViewMessage};

use crate::services::coincube::*;

#[derive(Debug, Copy, Clone, PartialEq)]
pub enum BuyOrSell {
    Sell,
    Buy,
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
    /// Registers a new user (email only)
    Register { email: String, loading: bool },
    /// User OTP verification UI
    OtpVerification {
        email: String,
        otp: String,
        sending: bool,
        is_signup: bool,
        cooldown: u8,
    },
    /// Logs in a user to their existing coincube account (email only)
    Login { email: String, loading: bool },
    /// Sets the mode for the panel before initializing the sub-states
    ModeSelect { buy_or_sell: Option<BuyOrSell> },
    /// Nigeria, Kenya and South Africa, ie Mavapay supported countries
    Mavapay(super::mavapay::MavapayState),
    /// Utilize Meld for countries not supported by Mavapay
    Meld(super::meld::MeldState),
}

impl BuySellFlowState {
    pub fn name(&self) -> &'static str {
        match self {
            BuySellFlowState::DetectingLocation(..) => "DetectingLocation",
            BuySellFlowState::Register { .. } => "Register",
            BuySellFlowState::OtpVerification { .. } => "OtpVerification",
            BuySellFlowState::Login { .. } => "Login",
            BuySellFlowState::ModeSelect { .. } => "ModeSelect",
            BuySellFlowState::Mavapay(..) => "Mavapay",
            BuySellFlowState::Meld { .. } => "Meld",
        }
    }
}

pub struct BuySellPanel {
    // Runtime state machine - determines which flow is liquid
    pub step: BuySellFlowState,

    // Common fields (always present)
    pub network: bitcoin::Network,

    // services used by several buysell providers
    pub coincube_client: crate::services::coincube::CoincubeClient,
    pub breez_client: std::sync::Arc<crate::app::breez_liquid::BreezClient>,
    pub detected_country: Option<&'static crate::services::coincube::Country>,

    // coincube session information, restored from OS keyring
    pub login: Option<LoginResponse>,

    // only really useful for address generation
    pub wallet: std::sync::Arc<crate::app::wallet::Wallet>,
}

impl BuySellPanel {
    pub fn new(
        network: bitcoin::Network,
        wallet: std::sync::Arc<crate::app::wallet::Wallet>,
        breez_client: std::sync::Arc<crate::app::breez_liquid::BreezClient>,
    ) -> Self {
        BuySellPanel {
            // Start in detecting location state
            step: BuySellFlowState::DetectingLocation(false),
            wallet,
            network,
            // API state
            coincube_client: crate::services::coincube::CoincubeClient::new(),
            breez_client,
            detected_country: None,
            login: None,
        }
    }

    pub fn view<'a>(&'a self) -> iced::Element<'a, ViewMessage, theme::Theme> {
        let column = {
            let column = Column::new()
                // COINCUBE branding
                .push(
                    Row::new()
                        .push(coincube_ui::image::coincube_wordmark::<ViewMessage>(20.0))
                        .push(Space::new().width(Length::Fixed(8.0)))
                        .push(text::h5_regular("| BUY/SELL").color(color::GREY_3))
                        .align_y(Alignment::Center),
                )
                // render flow state
                .push(match &self.step {
                    // user management
                    BuySellFlowState::Login { .. } => self.login_ux(),
                    BuySellFlowState::Register { .. } => self.registration_ux(),
                    BuySellFlowState::OtpVerification { .. } => self.otp_verification_ux(),

                    BuySellFlowState::DetectingLocation(..) => self.geolocation_ux(),
                    BuySellFlowState::ModeSelect { .. } => self.mode_select_ux(),

                    // mavapay
                    BuySellFlowState::Mavapay(state) => super::mavapay::ui::form(state),

                    // meld
                    BuySellFlowState::Meld(state) => state.view(),
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
        use coincube_ui::component::spinner;
        use std::time::Duration;

        let BuySellFlowState::Login { email, loading } = &self.step else {
            unreachable!();
        };

        let valid_email = email.contains('.') && email.contains('@') && email.len() >= 5;

        let submit_button = if *loading {
            iced::widget::button(
                Container::new(
                    Row::new()
                        .spacing(5)
                        .align_y(Alignment::Center)
                        .push(text::text("Signing In").size(16))
                        .push(
                            Container::new(spinner::typing_text_carousel(
                                "...",
                                true,
                                Duration::from_millis(500),
                                |s| text::text(s).size(16),
                            ))
                            .width(Length::Fixed(20.0)),
                        ),
                )
                .center_x(Length::Fill)
                .center_y(Length::Fill),
            )
            .width(Length::Fill)
            .height(Length::Fixed(44.0))
            .style(theme::button::primary)
        } else {
            button::primary(None, "Continue")
                .on_press_maybe(valid_email.then_some(BuySellMessage::SubmitLogin))
                .width(Length::Fill)
        };

        let col = iced::widget::column![
            // header
            text::h3("Sign in to your account").style(theme::text::primary),
            Space::new().height(Length::Fixed(35.0)),
            // input field
            text_input("Email", email)
                .on_input(BuySellMessage::EmailChanged)
                .on_submit_maybe((!*loading && valid_email).then_some(BuySellMessage::SubmitLogin))
                .size(16)
                .padding(15),
            Space::new().height(Length::Fixed(15.0)),
            // submit button
            submit_button,
            Space::new().height(Length::Fixed(10.0)),
            // separator
            container(Space::new().height(Length::Fixed(3.0)).width(Length::Fill))
                .style(theme::container::border),
            Space::new().height(Length::Fixed(5.0)),
            // sign-up redirect
            iced::widget::button(
                text::p2_regular("Don't have an account? Sign up").color(color::ORANGE),
            )
            .style(theme::button::link)
            .on_press(BuySellMessage::CreateNewAccount)
        ]
        .align_x(Alignment::Center)
        .spacing(2)
        .max_width(500)
        .width(Length::Fill);

        let elem: iced::Element<BuySellMessage, theme::Theme> = col.into();
        elem.map(ViewMessage::BuySell)
    }

    fn registration_ux<'a>(self: &'a BuySellPanel) -> iced::Element<'a, ViewMessage, theme::Theme> {
        use coincube_ui::component::spinner;
        use std::time::Duration;

        let BuySellFlowState::Register { email, loading } = &self.step else {
            unreachable!();
        };

        let valid_email = email.contains('.') && email.contains('@') && email.len() >= 5;

        let submit_button: iced::Element<ViewMessage, theme::Theme> = if *loading {
            iced::widget::button(
                Container::new(
                    Row::new()
                        .spacing(5)
                        .align_y(Alignment::Center)
                        .push(text::text("Signing up").size(16))
                        .push(
                            Container::new(spinner::typing_text_carousel(
                                "...",
                                true,
                                Duration::from_millis(500),
                                |s| text::text(s).size(16),
                            ))
                            .width(Length::Fixed(20.0)),
                        ),
                )
                .center_x(Length::Fill)
                .center_y(Length::Fill),
            )
            .width(Length::Fill)
            .height(Length::Fixed(44.0))
            .style(theme::button::primary)
            .into()
        } else {
            button::primary(None, "Continue")
                .on_press_maybe(
                    valid_email.then_some(ViewMessage::BuySell(BuySellMessage::SubmitRegistration)),
                )
                .width(Length::Fill)
                .into()
        };

        // TODO: include form validation messages
        iced::widget::column![
            // Top bar with previous
            Button::new(
                Row::new()
                    .push(previous_icon().style(theme::text::secondary))
                    .push(Space::new().width(Length::Fixed(5.0)))
                    .push(text::p1_medium("Previous").style(theme::text::secondary))
                    .spacing(5)
                    .align_y(Alignment::Center),
            )
            .style(theme::button::transparent)
            .on_press_maybe(
                (!*loading).then_some(ViewMessage::BuySell(BuySellMessage::ResetWidget))
            ),
            Space::new().height(Length::Fixed(10.0)),
            // Title and subtitle
            iced::widget::column![
                text::h3("Create an Account").style(theme::text::primary),
                text::p2_regular("Create a COINCUBE account to access Buy/Sell and other features")
                    .style(theme::text::secondary)
            ]
            .spacing(10)
            .align_x(Alignment::Center),
            Space::new().height(Length::Fixed(20.0)),
            // Email Input
            text_input("Email Address", email)
                .on_input(|i| ViewMessage::BuySell(BuySellMessage::EmailChanged(i)))
                .on_submit_maybe(
                    (!*loading && valid_email)
                        .then_some(ViewMessage::BuySell(BuySellMessage::SubmitRegistration)),
                )
                .size(16)
                .padding(15),
            Space::new().height(Length::Fixed(20.0)),
            submit_button,
        ]
        .align_x(Alignment::Center)
        .spacing(5)
        .max_width(500)
        .width(Length::Fill)
        .into()
    }

    fn otp_verification_ux<'a>(
        self: &'a BuySellPanel,
    ) -> iced::Element<'a, ViewMessage, theme::Theme> {
        use coincube_ui::component::spinner;
        use std::time::Duration;

        let BuySellFlowState::OtpVerification {
            email,
            otp,
            sending,
            is_signup: _,
            cooldown,
        } = &self.step
        else {
            unreachable!()
        };

        // Top bar with previous
        let top_bar = Row::new()
            .push(
                Button::new(
                    Row::new()
                        .push(previous_icon().style(theme::text::secondary))
                        .push(Space::new().width(Length::Fixed(5.0)))
                        .push(text::text("Previous").style(theme::text::secondary))
                        .spacing(5)
                        .align_y(Alignment::Center),
                )
                .style(theme::button::transparent)
                .on_press_maybe((!*sending).then_some(BuySellMessage::ResetWidget)),
            )
            .align_y(Alignment::Center);

        // Widget title
        let title =
            text::p2_regular("Enter the OTP sent to your email").style(theme::text::primary);

        // Email display
        let email_display = Column::new()
            .push(text::p2_regular(email).style(theme::text::primary))
            .spacing(10)
            .align_x(Alignment::Center);

        // OTP Input
        let otp_input = text_input("Enter OTP code", otp)
            .on_input(BuySellMessage::OtpChanged)
            .on_submit_maybe((!otp.is_empty() && !*sending).then_some(BuySellMessage::VerifyOtp))
            .size(16)
            .padding(15);

        // Verify button with spinner
        let verify_button: iced::Element<BuySellMessage, theme::Theme> = if *sending {
            iced::widget::button(
                Container::new(
                    Row::new()
                        .spacing(5)
                        .align_y(Alignment::Center)
                        .push(text::text("Verifying").size(16))
                        .push(
                            Container::new(spinner::typing_text_carousel(
                                "...",
                                true,
                                Duration::from_millis(500),
                                |s| text::text(s).size(16),
                            ))
                            .width(Length::Fixed(20.0)),
                        ),
                )
                .center_x(Length::Fill)
                .center_y(Length::Fill),
            )
            .width(Length::FillPortion(1))
            .height(Length::Fixed(44.0))
            .style(theme::button::primary)
            .into()
        } else {
            button::primary(None, "Verify OTP")
                .on_press_maybe((!otp.is_empty()).then_some(BuySellMessage::VerifyOtp))
                .width(Length::FillPortion(1))
                .into()
        };

        // Action buttons
        let resend_enabled = !*sending && *cooldown == 0;
        let resend_button: iced::Element<BuySellMessage, theme::Theme> = if *cooldown > 0 {
            let label = format!("Resend in {}s", cooldown);
            iced::widget::button(
                Container::new(
                    Row::new()
                        .push(email_icon().style(theme::text::secondary))
                        .push(text::text(label).size(14))
                        .spacing(10)
                        .align_y(Alignment::Center),
                )
                .center_x(Length::Fill)
                .center_y(Length::Fill),
            )
            .style(theme::button::secondary)
            .width(Length::FillPortion(1))
            .height(Length::Fixed(44.0))
            .into()
        } else {
            button::secondary(Some(email_icon()), "Resend OTP")
                .on_press_maybe(resend_enabled.then_some(BuySellMessage::SendOtp))
                .width(Length::FillPortion(1))
                .into()
        };

        let action_buttons = Row::new()
            .push(resend_button)
            .push(Space::new().width(Length::Fixed(10.0)))
            .push(verify_button)
            .spacing(10)
            .align_y(Alignment::Center);

        let col = iced::widget::column![
            top_bar,
            Space::new().height(Length::Fixed(10.0)),
            title,
            Space::new().height(Length::Fixed(30.0)),
            email_display,
            Space::new().height(Length::Fixed(20.0)),
            otp_input,
            Space::new().height(Length::Fixed(20.0)),
            action_buttons,
        ]
        .align_x(Alignment::Center)
        .spacing(5)
        .max_width(500)
        .width(Length::Fill);

        let elem: iced::Element<BuySellMessage, theme::Theme> = col.into();
        elem.map(ViewMessage::BuySell)
    }

    fn mode_select_ux<'a>(&'a self) -> iced::Element<'a, ViewMessage, theme::Theme> {
        let BuySellFlowState::ModeSelect { buy_or_sell } = &self.step else {
            unreachable!()
        };

        iced::widget::column![
            // mode select ux
            iced::widget::column![
                button::secondary(Some(bitcoin_icon()), "Buy Bitcoin using Fiat Currencies",)
                    .on_press(ViewMessage::BuySell(BuySellMessage::SelectBuyOrSell(
                        BuyOrSell::Buy,
                    )))
                    .style({
                        match buy_or_sell {
                            Some(BuyOrSell::Buy) => coincube_ui::theme::button::primary,
                            _ => coincube_ui::theme::button::secondary,
                        }
                    })
                    .padding(35)
                    .width(iced::Length::Fill),
                button::secondary(Some(dollar_icon()), "Sell Bitcoin to a Fiat Currency")
                    .on_press(ViewMessage::BuySell(BuySellMessage::SelectBuyOrSell(
                        BuyOrSell::Sell,
                    )))
                    .style({
                        match buy_or_sell {
                            Some(BuyOrSell::Sell) => coincube_ui::theme::button::primary,
                            _ => coincube_ui::theme::button::secondary,
                        }
                    })
                    .padding(35)
                    .width(iced::Length::Fill),
            ]
            .spacing(15)
            .padding(5),
            // separator
            iced::widget::container(Space::new().height(3))
                .style(|_| {
                    iced::widget::container::background(iced::Background::Color(color::GREY_3))
                })
                .width(Length::Fill),
            // history view
            buy_or_sell.is_none().then(|| {
                iced::widget::row![
                    button::secondary(Some(history_icon()), "View Order History")
                        .on_press(ViewMessage::BuySell(BuySellMessage::ViewHistory))
                        .width(iced::Length::Fill),
                    button::secondary(Some(escape_icon()), "Log Out")
                        .on_press(ViewMessage::BuySell(BuySellMessage::LogOut))
                        .width(iced::Length::Fill)
                ]
                .spacing(10)
            }),
            // submit
            buy_or_sell.is_some().then(|| {
                button::secondary(Some(globe_icon()), "Continue")
                    .on_press_maybe(
                        self.detected_country
                            .is_some()
                            .then_some(ViewMessage::BuySell(BuySellMessage::StartSession)),
                    )
                    .width(iced::Length::Fill)
            })
        ]
        .align_x(Alignment::Center)
        .spacing(12)
        .max_width(640)
        .width(Length::Fill)
        .into()
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
                        self.detected_country,
                        |c| {
                            let static_country = crate::services::coincube::get_countries()
                                .iter()
                                .find(|cs| cs.code == c.code)
                                .unwrap();
                            BuySellMessage::CountryDetected(Ok(static_country))
                        },
                    )
                    .padding(10)
                    .placeholder("Select Country: "),
                )
                .align_x(Alignment::Center)
                .width(Length::Fill),
            false => Column::new()
                .push(Space::new().height(Length::Fixed(30.0)))
                .push(text::p1_bold("Detecting your location...").style(theme::text::primary))
                .push(Space::new().height(Length::Fixed(20.0)))
                .push(
                    text::text("Please wait...")
                        .size(14)
                        .style(theme::text::secondary),
                )
                .align_x(Alignment::Center)
                .spacing(10)
                .max_width(500)
                .width(Length::Fill),
        };

        let elem: iced::Element<BuySellMessage, theme::Theme> = col.into();
        elem.map(ViewMessage::BuySell)
    }
}

use iced::{
    widget::{container, pick_list, text_input, Space},
    Alignment, Length,
};

use coincube_core::miniscript::bitcoin;
use coincube_ui::{
    color,
    component::{button, card, text},
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
    /// Renders an interface to either generate a new address for bitcoin deposit, or skip to selling BTC
    Initialization {
        modal: app::state::vault::receive::Modal,
        buy_or_sell_selected: Option<bool>,
        buy_or_sell: Option<BuyOrSell>, // `buy` mode always has an address included for deposit
    },
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
            BuySellFlowState::Initialization { .. } => "Initialization",
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
    ) -> Self {
        BuySellPanel {
            // Start in detecting location state
            step: BuySellFlowState::DetectingLocation(false),
            wallet,
            network,
            // API state
            coincube_client: crate::services::coincube::CoincubeClient::new(None),
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
                        .push(
                            Row::new()
                                .push(text::h4_bold("COIN").color(color::ORANGE))
                                .push(text::h4_bold("CUBE").color(color::WHITE))
                                .spacing(0),
                        )
                        .push(Space::new().width(Length::Fixed(8.0)))
                        .push(text::h5_regular("BUY/SELL").color(color::GREY_3))
                        .align_y(Alignment::Center),
                )
                // render flow state
                .push({
                    match &self.step {
                        // user management
                        BuySellFlowState::Login { .. } => self.login_ux(),
                        BuySellFlowState::Register { .. } => self.registration_ux(),
                        BuySellFlowState::OtpVerification { .. } => self.otp_verification_ux(),

                        // init
                        BuySellFlowState::DetectingLocation(..) => self.geolocation_ux(),
                        BuySellFlowState::Initialization { .. } => self.initialization_ux(),

                        // mavapay
                        BuySellFlowState::Mavapay(state) => super::mavapay::ui::form(state),

                        // meld
                        BuySellFlowState::Meld(state) => state.view(&self.network),
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
        use coincube_ui::component::spinner;
        use std::time::Duration;

        let BuySellFlowState::Login { email, loading } = &self.step else {
            unreachable!();
        };

        let valid_email = email.contains('.') && email.contains('@');

        let submit_button: iced::Element<BuySellMessage, theme::Theme> = if *loading {
            iced::widget::button(
                Container::new(
                    Row::new()
                        .spacing(5)
                        .align_y(Alignment::Center)
                        .push(text::text("Signing in").size(16))
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
                .on_press_maybe(valid_email.then_some(BuySellMessage::SubmitLogin))
                .width(Length::Fill)
                .into()
        };

        let col = iced::widget::column![
            // header
            text::h3("Sign in to your account").color(color::WHITE),
            Space::new().height(Length::Fixed(35.0)),
            // input field
            text_input("Email", email)
                .on_input(BuySellMessage::EmailChanged)
                .on_submit_maybe((!*loading && valid_email).then_some(BuySellMessage::SubmitLogin),)
                .size(16)
                .padding(15),
            Space::new().height(Length::Fixed(15.0)),
            // submit button
            submit_button,
            Space::new().height(Length::Fixed(10.0)),
            // separator
            container(Space::new().height(Length::Fixed(3.0)).width(Length::Fill))
                .style(|_| { color::GREY_6.into() }),
            Space::new().height(Length::Fixed(5.0)),
            // sign-up redirect
            iced::widget::button(
                text::p2_regular("Don't have an account? Sign up").color(color::BLUE),
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

        let valid_email = email.contains('.') && email.contains('@');

        let submit_button: iced::Element<BuySellMessage, theme::Theme> = if *loading {
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
                .on_press_maybe(valid_email.then_some(BuySellMessage::SubmitRegistration))
                .width(Length::Fill)
                .into()
        };

        // TODO: include form validation messages
        let col = iced::widget::column![
            // Top bar with previous
            Button::new(
                Row::new()
                    .push(previous_icon().color(color::GREY_2))
                    .push(Space::new().width(Length::Fixed(5.0)))
                    .push(text::p1_medium("Previous").color(color::GREY_2))
                    .spacing(5)
                    .align_y(Alignment::Center),
            )
            .style(|_, _| iced::widget::button::Style {
                background: None,
                text_color: color::GREY_2,
                border: iced::Border::default(),
                shadow: iced::Shadow::default(),
                snap: true
            })
            .on_press_maybe((!*loading).then_some(BuySellMessage::ResetWidget)),
            Space::new().height(Length::Fixed(10.0)),
            // Title and subtitle
            iced::widget::column![
                text::h3("Create an Account").color(color::WHITE),
                text::p2_regular(
                    "Create a COINCUBE account to access Buy/Sell and other features."
                )
                .color(color::GREY_3)
            ]
            .spacing(10)
            .align_x(Alignment::Center),
            Space::new().height(Length::Fixed(20.0)),
            // Email Input
            text_input("Email Address", email)
                .on_input(BuySellMessage::EmailChanged)
                .on_submit_maybe(
                    (!*loading && valid_email).then_some(BuySellMessage::SubmitRegistration),
                )
                .size(16)
                .padding(15),
            Space::new().height(Length::Fixed(20.0)),
            submit_button,
        ]
        .align_x(Alignment::Center)
        .spacing(5)
        .max_width(500)
        .width(Length::Fill);

        let elem: iced::Element<BuySellMessage, theme::Theme> = col.into();
        elem.map(ViewMessage::BuySell)
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
                        .push(previous_icon().color(color::GREY_2))
                        .push(Space::new().width(Length::Fixed(5.0)))
                        .push(text::text("Previous").color(color::GREY_2))
                        .spacing(5)
                        .align_y(Alignment::Center),
                )
                .style(|_, _| iced::widget::button::Style {
                    background: None,
                    text_color: color::GREY_2,
                    border: iced::Border::default(),
                    shadow: iced::Shadow::default(),
                    snap: true,
                })
                .on_press_maybe((!*sending).then_some(BuySellMessage::ResetWidget)),
            )
            .align_y(Alignment::Center);

        // Widget title
        let title = text::p2_regular("Enter the OTP sent to your email").color(color::WHITE);

        // Email display
        let email_display = Column::new()
            .push(text::p2_regular(email).color(color::WHITE))
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
            Some(BuyOrSell::Buy { address }) => {
                let address_text = address.to_string();

                Column::new()
                    .push(
                        button::transparent(Some(previous_icon()), "Previous")
                            .width(Length::Shrink)
                            .on_press(ViewMessage::BuySell(BuySellMessage::ResetWidget)),
                    )
                    .push(
                        Column::new()
                            .push(Space::new().height(Length::Fixed(15.0)))
                            .push(
                                text::p1_italic(
                                    "Bitcoin will be deposited in the following address",
                                )
                                .color(color::GREY_2),
                            )
                            .push(
                                card::simple(
                                    Column::new()
                                        .push(
                                            Container::new(
                                                scrollable(
                                                    Column::new()
                                                        .push(
                                                            Space::new()
                                                                .height(Length::Fixed(10.0)),
                                                        )
                                                        .push(
                                                            text::Text::small(text::p2_regular(
                                                                &address_text,
                                                            ))
                                                            .style(theme::text::secondary),
                                                        )
                                                        // Space between the address and the scrollbar
                                                        .push(
                                                            Space::new()
                                                                .height(Length::Fixed(10.0)),
                                                        ),
                                                )
                                                .direction(scrollable::Direction::Horizontal(
                                                    scrollable::Scrollbar::new()
                                                        .width(2)
                                                        .scroller_width(2),
                                                )),
                                            )
                                            .width(Length::Fill),
                                        )
                                        .push(
                                            Row::new()
                                                .push(
                                                    button::secondary(
                                                        None,
                                                        "Verify on hardware device",
                                                    )
                                                    .on_press(ViewMessage::Select(0)),
                                                )
                                                .push(Space::new().width(Length::Fill))
                                                .push(
                                                    Button::new(
                                                        qr_code_icon()
                                                            .style(theme::text::secondary),
                                                    )
                                                    .on_press(ViewMessage::ShowQrCode(0))
                                                    .style(theme::button::transparent_border),
                                                )
                                                .push(
                                                    Button::new(
                                                        clipboard_icon()
                                                            .style(theme::text::secondary),
                                                    )
                                                    .on_press(ViewMessage::Clipboard(address_text))
                                                    .style(theme::button::transparent_border),
                                                )
                                                .align_y(Alignment::Center),
                                        )
                                        .spacing(10),
                                )
                                .width(Length::Fill),
                            ),
                    )
                    .push(
                        button::primary(Some(globe_icon()), "Continue")
                            .on_press_maybe(
                                self.detected_country
                                    .is_some()
                                    .then_some(ViewMessage::BuySell(BuySellMessage::StartSession)),
                            )
                            .width(iced::Length::Fill),
                    )
                    .spacing(12)
                    .max_width(640)
                    .width(Length::Fill)
            }
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
                            .padding(35)
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
                            .padding(35)
                            .width(iced::Length::Fill),
                        )
                        .spacing(15)
                        .padding(5)
                })
                .push(
                    iced::widget::container(Space::new().height(3))
                        .style(|_| {
                            iced::widget::container::background(iced::Background::Color(
                                color::GREY_3,
                            ))
                        })
                        .width(Length::Fill),
                )
                .push({
                    (matches!(buy_or_sell_selected, Some(true))).then(|| {
                        button::secondary(Some(plus_icon()), "Generate New Address")
                            .on_press(ViewMessage::BuySell(BuySellMessage::CreateNewAddress))
                            .width(iced::Length::Fill)
                    })
                })
                .push({
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
                .push({
                    buy_or_sell_selected.is_none().then(|| {
                        iced::widget::row![
                            button::secondary(Some(history_icon()), "View Order History")
                                .on_press(ViewMessage::BuySell(BuySellMessage::ViewHistory))
                                .width(iced::Length::Fill),
                            button::secondary(Some(escape_icon()), "Log Out")
                                .on_press(ViewMessage::BuySell(BuySellMessage::LogOut))
                                .width(iced::Length::Fill)
                        ]
                        .spacing(10)
                    })
                })
                .align_x(Alignment::Center)
                .spacing(12)
                .max_width(640)
                .width(Length::Fill),
        };

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
                .push(text::p1_bold("Detecting your location...").color(color::WHITE))
                .push(Space::new().height(Length::Fixed(20.0)))
                .push(text::text("Please wait...").size(14).color(color::GREY_3))
                .align_x(Alignment::Center)
                .spacing(10)
                .max_width(500)
                .width(Length::Fill),
        };

        let elem: iced::Element<BuySellMessage, theme::Theme> = col.into();
        elem.map(ViewMessage::BuySell)
    }
}

use coincube_ui::locale::copy as localized_copy;
use coincube_ui::localized_format;
use iced::{
    widget::{pick_list, text_input, Space},
    Alignment, Length,
};

use coincube_core::miniscript::bitcoin;
use coincube_ui::{
    component::{button, text},
    icon::*,
    theme,
    theme::tenshu::surface,
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
    /// The Cube's Spark wallet: Mavapay buys receive into it and sells are
    /// paid from it. The panel isn't built without one.
    pub spark: std::sync::Arc<crate::app::wallets::SparkBackend>,
    pub detected_country: Option<&'static crate::services::coincube::Country>,

    // coincube session information, restored from OS keyring
    pub login: Option<LoginResponse>,

    // only really useful for address generation
    pub wallet: std::sync::Arc<crate::app::wallet::Wallet>,

    /// Session changes (`SetLoginState`, `LogOut`) that arrived while a sell
    /// payment was in flight, applied in order once it answers.
    pub deferred_session: Vec<crate::app::view::BuySellMessage>,
}

impl BuySellPanel {
    pub fn new(
        network: bitcoin::Network,
        wallet: std::sync::Arc<crate::app::wallet::Wallet>,
        spark: std::sync::Arc<crate::app::wallets::SparkBackend>,
    ) -> Self {
        BuySellPanel {
            // Start in detecting location state
            step: BuySellFlowState::DetectingLocation(false),
            wallet,
            network,
            // API state
            coincube_client: crate::services::coincube::CoincubeClient::new(),
            spark,
            detected_country: None,
            login: None,
            deferred_session: Vec::new(),
        }
    }

    /// A sell checkout's Spark payment is unresolved (with the bridge, or
    /// checked without a verdict): the flow must not be replaced, nor a held
    /// session change applied, until it is. See [`SellFulfilment::in_flight`].
    ///
    /// [`SellFulfilment::in_flight`]: super::mavapay::SellFulfilment::in_flight
    pub fn sell_payment_in_flight(&self) -> bool {
        matches!(
            &self.step,
            BuySellFlowState::Mavapay(state)
                if matches!(
                    state.steps.last(),
                    Some(super::mavapay::MavapayFlowStep::Checkout { sell, .. })
                        if sell.fulfilment.in_flight()
                )
        )
    }

    /// The held-back session changes, re-dispatched in the order they were
    /// held, once no sell payment is unresolved. `None` while one still is,
    /// or when nothing was held.
    ///
    /// One chained task, not a batch: `Task::batch` runs its tasks
    /// concurrently with no ordering guarantee, so a held `LogOut` could be
    /// applied before a `SetLoginState` queued ahead of it, or the other way
    /// round, and leave the session in the state the user didn't end in.
    pub fn take_deferred_session(&mut self) -> Option<iced::Task<crate::app::message::Message>> {
        if self.sell_payment_in_flight() {
            return None;
        }
        std::mem::take(&mut self.deferred_session)
            .into_iter()
            .map(|msg| {
                iced::Task::done(crate::app::message::Message::View(
                    crate::app::view::Message::BuySell(msg),
                ))
            })
            .reduce(iced::Task::chain)
    }

    pub fn view<'a>(&'a self, mask_amounts: bool) -> iced::Element<'a, ViewMessage, theme::Theme> {
        match &self.step {
            BuySellFlowState::DetectingLocation(..)
            | BuySellFlowState::Login { .. }
            | BuySellFlowState::Register { .. }
            | BuySellFlowState::OtpVerification { .. }
            | BuySellFlowState::ModeSelect { .. } => {
                Self::entry_view(&self.step, self.detected_country)
            }
            BuySellFlowState::Mavapay(state) => {
                Self::provider_view(super::mavapay::ui::form(state, mask_amounts))
            }
            BuySellFlowState::Meld(state) => Self::provider_view(state.view()),
        }
    }

    fn provider_view<'a>(
        content: iced::Element<'a, ViewMessage, theme::Theme>,
    ) -> iced::Element<'a, ViewMessage, theme::Theme> {
        Container::new(
            Column::new()
                .push(text::h2(localized_copy("Buy/Sell")).style(theme::text::primary))
                .push(content)
                .spacing(16)
                .width(Length::Fill),
        )
        .width(Length::Fill)
        .max_width(760)
        .padding([24, 0])
        .align_y(Alignment::Start)
        .align_x(Alignment::Center)
        .into()
    }

    fn entry_view<'a>(
        step: &'a BuySellFlowState,
        detected_country: Option<&'static Country>,
    ) -> iced::Element<'a, ViewMessage, theme::Theme> {
        let content = match step {
            BuySellFlowState::DetectingLocation(..) => Self::geolocation_ux(step, detected_country),
            BuySellFlowState::Login { .. } => Self::login_ux(step),
            BuySellFlowState::Register { .. } => Self::registration_ux(step),
            BuySellFlowState::OtpVerification { .. } => Self::otp_verification_ux(step),
            BuySellFlowState::ModeSelect { .. } => Self::mode_select_ux(step, detected_country),
            _ => unreachable!("entry presenter only accepts entry states"),
        };
        let mut column = Column::new()
            .push(text::h2(localized_copy("Buy/Sell")).style(theme::text::primary))
            .push(Self::entry_progress(step))
            .push(
                Container::new(content)
                    .padding(22)
                    .width(Length::Fill)
                    .style(surface::card),
            )
            .spacing(16)
            .width(Length::Fill);

        if let Some(country) = detected_country {
            let provider = if crate::services::mavapay::mavapay_supported(country.code) {
                "Mavapay"
            } else {
                "Meld"
            };
            column = column.push(
                Row::new()
                    .push(
                        text::p2_regular(localized_format!(
                            "Country  {}",
                            "País  {}",
                            country.name
                        ))
                        .style(theme::text::secondary),
                    )
                    .push(
                        text::p2_regular(localized_format!(
                            "Provider  {provider}",
                            "Proveedor  {provider}"
                        ))
                        .style(theme::text::secondary),
                    )
                    .spacing(24),
            );
        }

        Container::new(column)
            .width(Length::Fill)
            .max_width(760)
            .padding([24, 0])
            .align_y(Alignment::Start)
            .align_x(Alignment::Center)
            .into()
    }

    fn entry_progress<'a>(step: &BuySellFlowState) -> iced::Element<'a, ViewMessage, theme::Theme> {
        let current = match step {
            BuySellFlowState::DetectingLocation(..) => 0,
            BuySellFlowState::Login { .. }
            | BuySellFlowState::Register { .. }
            | BuySellFlowState::OtpVerification { .. } => 1,
            BuySellFlowState::ModeSelect { .. } => 2,
            _ => unreachable!("progress is only shown for entry states"),
        };

        let mut row = Row::new().spacing(12).width(Length::Fill);
        for (index, label) in ["Location", "Sign in", "Buy or sell"].iter().enumerate() {
            let label = localized_copy(label);
            let step: iced::Element<'a, ViewMessage, theme::Theme> = if index < current {
                Row::new()
                    .push(check_icon().style(theme::text::success))
                    .push(text::p2_regular(label).style(theme::text::secondary))
                    .spacing(4)
                    .align_y(Alignment::Center)
                    .into()
            } else if index == current {
                text::p2_bold(format!("{}  {label}", index + 1))
                    .style(theme::tenshu::text::accent)
                    .into()
            } else {
                text::p2_regular(format!("{}  {label}", index + 1))
                    .style(theme::text::secondary)
                    .into()
            };
            row = row.push(Container::new(step).width(Length::FillPortion(1)));
        }
        row.into()
    }
}

// TODO: Use labels instead of placeholders for all input forms
impl BuySellPanel {
    fn login_ux<'a>(step: &'a BuySellFlowState) -> iced::Element<'a, ViewMessage, theme::Theme> {
        use coincube_ui::component::spinner;
        use std::time::Duration;

        let BuySellFlowState::Login { email, loading } = step else {
            unreachable!();
        };

        let valid_email = email.contains('.') && email.contains('@') && email.len() >= 5;

        let submit_button = if *loading {
            iced::widget::button(
                Container::new(
                    Row::new()
                        .spacing(5)
                        .align_y(Alignment::Center)
                        .push(text::text(localized_copy("Signing In")).size(16))
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
            button::primary(None, localized_copy("Continue"))
                .on_press_maybe(valid_email.then_some(BuySellMessage::SubmitLogin))
                .width(Length::Fill)
        };

        let col = iced::widget::column![
            text::h3(localized_copy("Sign in to your account")).style(theme::text::primary),
            text::p2_medium(localized_copy("Email")).style(theme::text::primary),
            text_input(localized_copy("Email"), email)
                .on_input(BuySellMessage::EmailChanged)
                .on_submit_maybe((!*loading && valid_email).then_some(BuySellMessage::SubmitLogin))
                .size(16)
                .padding(15),
            submit_button,
            iced::widget::button(
                text::p2_regular(localized_copy("Don't have an account? Sign up"))
                    .style(theme::tenshu::text::accent),
            )
            .style(theme::button::link)
            .on_press(BuySellMessage::CreateNewAccount)
        ]
        .align_x(Alignment::Start)
        .spacing(16)
        .width(Length::Fill);

        let elem: iced::Element<BuySellMessage, theme::Theme> = col.into();
        elem.map(ViewMessage::BuySell)
    }

    fn registration_ux<'a>(
        step: &'a BuySellFlowState,
    ) -> iced::Element<'a, ViewMessage, theme::Theme> {
        use coincube_ui::component::spinner;
        use std::time::Duration;

        let BuySellFlowState::Register { email, loading } = step else {
            unreachable!();
        };

        let valid_email = email.contains('.') && email.contains('@') && email.len() >= 5;

        let submit_button: iced::Element<ViewMessage, theme::Theme> = if *loading {
            iced::widget::button(
                Container::new(
                    Row::new()
                        .spacing(5)
                        .align_y(Alignment::Center)
                        .push(text::text(localized_copy("Signing up")).size(16))
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
            button::primary(None, localized_copy("Continue"))
                .on_press_maybe(
                    valid_email.then_some(ViewMessage::BuySell(BuySellMessage::SubmitRegistration)),
                )
                .width(Length::Fill)
                .into()
        };

        iced::widget::column![
            button::secondary(Some(previous_icon()), localized_copy("Back"))
                .width(Length::Fixed(150.0))
                .on_press_maybe(
                    (!*loading).then_some(ViewMessage::BuySell(BuySellMessage::ResetWidget))
                ),
            text::h3(localized_copy("Create an Account")).style(theme::text::primary),
            text::p2_regular(localized_copy(
                "Create a COINCUBE account to access Buy/Sell and other features",
            ))
            .style(theme::text::secondary),
            text::p2_medium(localized_copy("Email Address")).style(theme::text::primary),
            text_input(localized_copy("Email Address"), email)
                .on_input(|i| ViewMessage::BuySell(BuySellMessage::EmailChanged(i)))
                .on_submit_maybe(
                    (!*loading && valid_email)
                        .then_some(ViewMessage::BuySell(BuySellMessage::SubmitRegistration)),
                )
                .size(16)
                .padding(15),
            submit_button,
        ]
        .align_x(Alignment::Start)
        .spacing(16)
        .width(Length::Fill)
        .into()
    }

    fn otp_verification_ux<'a>(
        step: &'a BuySellFlowState,
    ) -> iced::Element<'a, ViewMessage, theme::Theme> {
        use coincube_ui::component::spinner;
        use std::time::Duration;

        let BuySellFlowState::OtpVerification {
            email,
            otp,
            sending,
            is_signup: _,
            cooldown,
        } = step
        else {
            unreachable!()
        };

        let top_bar = Row::new()
            .push(
                button::secondary(Some(previous_icon()), localized_copy("Back"))
                    .width(Length::Fixed(150.0))
                    .on_press_maybe((!*sending).then_some(BuySellMessage::ResetWidget)),
            )
            .align_y(Alignment::Center);

        let otp_input = text_input(localized_copy("Enter OTP code"), otp)
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
                        .push(text::text(localized_copy("Verifying")).size(16))
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
            button::primary(None, localized_copy("Verify OTP"))
                .on_press_maybe((!otp.is_empty()).then_some(BuySellMessage::VerifyOtp))
                .width(Length::FillPortion(1))
                .into()
        };

        // Action buttons
        let resend_enabled = !*sending && *cooldown == 0;
        let resend_button: iced::Element<BuySellMessage, theme::Theme> = if *cooldown > 0 {
            let label = localized_format!("Resend in {}s", "Reenviar en {} s", cooldown);
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
            button::secondary(Some(email_icon()), localized_copy("Resend OTP"))
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
            text::h3(localized_copy("Enter the OTP sent to your email"))
                .style(theme::text::primary),
            text::p2_regular(email).style(theme::text::secondary),
            text::p2_medium(localized_copy("OTP code")).style(theme::text::primary),
            otp_input,
            action_buttons,
        ]
        .align_x(Alignment::Start)
        .spacing(16)
        .width(Length::Fill);

        let elem: iced::Element<BuySellMessage, theme::Theme> = col.into();
        elem.map(ViewMessage::BuySell)
    }

    fn mode_select_ux<'a>(
        step: &'a BuySellFlowState,
        detected_country: Option<&'static Country>,
    ) -> iced::Element<'a, ViewMessage, theme::Theme> {
        let BuySellFlowState::ModeSelect { buy_or_sell } = step else {
            unreachable!()
        };

        iced::widget::column![
            text::h3(localized_copy("Buy or sell")).style(theme::text::primary),
            button::secondary(
                Some(bitcoin_icon()),
                localized_copy("Buy Bitcoin using Fiat Currencies"),
            )
            .on_press(ViewMessage::BuySell(BuySellMessage::SelectBuyOrSell(
                BuyOrSell::Buy
            )))
            .style(if *buy_or_sell == Some(BuyOrSell::Buy) {
                theme::button::primary
            } else {
                theme::button::secondary
            })
            .padding(18)
            .width(Length::Fill),
            button::secondary(
                Some(dollar_icon()),
                localized_copy("Sell Bitcoin to a Fiat Currency"),
            )
            .on_press(ViewMessage::BuySell(BuySellMessage::SelectBuyOrSell(
                BuyOrSell::Sell
            )))
            .style(if *buy_or_sell == Some(BuyOrSell::Sell) {
                theme::button::primary
            } else {
                theme::button::secondary
            })
            .padding(18)
            .width(Length::Fill),
            buy_or_sell.is_none().then(|| {
                iced::widget::row![
                    button::secondary(Some(history_icon()), localized_copy("View Order History"))
                        .on_press(ViewMessage::BuySell(BuySellMessage::ViewHistory))
                        .width(Length::Shrink),
                    button::secondary(Some(escape_icon()), localized_copy("Log Out"))
                        .on_press(ViewMessage::BuySell(BuySellMessage::LogOut))
                        .width(Length::Shrink)
                ]
                .spacing(10)
            }),
            buy_or_sell.is_some().then(|| {
                button::primary(Some(globe_icon()), localized_copy("Continue"))
                    .on_press_maybe(
                        detected_country
                            .is_some()
                            .then_some(ViewMessage::BuySell(BuySellMessage::StartSession)),
                    )
                    .width(Length::Fill)
            })
        ]
        .align_x(Alignment::Start)
        .spacing(16)
        .width(Length::Fill)
        .into()
    }

    fn geolocation_ux<'a>(
        step: &'a BuySellFlowState,
        detected_country: Option<&'static Country>,
    ) -> iced::Element<'a, ViewMessage, theme::Theme> {
        let BuySellFlowState::DetectingLocation(manual) = step else {
            unreachable!()
        };

        let col = match *manual {
            true => Column::new()
                .push(text::h3(localized_copy("Location")).style(theme::text::primary))
                .push(
                    text::p2_regular(localized_copy(
                        "Choose your country to find an available provider.",
                    ))
                    .style(theme::text::secondary),
                )
                .push(
                    text::p2_medium(localized_copy("Select Country:")).style(theme::text::primary),
                )
                .push(
                    pick_list(
                        crate::services::coincube::get_countries(),
                        detected_country,
                        |c| {
                            let static_country = crate::services::coincube::get_countries()
                                .iter()
                                .find(|cs| cs.code == c.code)
                                .unwrap();
                            BuySellMessage::CountryDetected(Ok(static_country))
                        },
                    )
                    .padding(10)
                    .placeholder(localized_copy("Select Country: ")),
                )
                .align_x(Alignment::Start)
                .spacing(16)
                .width(Length::Fill),
            false => Column::new()
                .push(text::h3(localized_copy("Location")).style(theme::text::primary))
                .push(
                    text::p1_bold(localized_copy("Detecting your location..."))
                        .style(theme::text::primary),
                )
                .push(
                    text::text(localized_copy("Please wait..."))
                        .size(14)
                        .style(theme::text::secondary),
                )
                .align_x(Alignment::Start)
                .spacing(16)
                .width(Length::Fill),
        };

        let elem: iced::Element<BuySellMessage, theme::Theme> = col.into();
        elem.map(ViewMessage::BuySell)
    }
}

#[cfg(test)]
mod entry_presenter_tests {
    use super::*;
    use crate::app::view::shared::landing::capture;

    fn country(code: &str) -> &'static Country {
        get_countries()
            .iter()
            .find(|country| country.code == code)
            .expect("synthetic country is available")
    }

    fn has_exact(inspection: &capture::Inspection, needle: &str) -> bool {
        inspection.text.iter().any(|(value, _)| value == needle)
    }

    /// Draws the production entry presenter with synthetic state. Set
    /// R06A_CAPTURE_DIR to write content-only PNGs from the software renderer.
    #[test]
    fn entry_states_show_only_completed_real_stages_at_compact_and_wide_sizes() {
        let scenarios = [
            (
                "detecting",
                BuySellFlowState::DetectingLocation(false),
                None,
                0,
            ),
            (
                "manual-country",
                BuySellFlowState::DetectingLocation(true),
                None,
                0,
            ),
            (
                "login",
                BuySellFlowState::Login {
                    email: "alex@example.test".to_owned(),
                    loading: false,
                },
                Some(country("US")),
                1,
            ),
            (
                "otp",
                BuySellFlowState::OtpVerification {
                    email: "alex@example.test".to_owned(),
                    otp: String::new(),
                    sending: false,
                    is_signup: false,
                    cooldown: 28,
                },
                Some(country("US")),
                1,
            ),
            (
                "mode",
                BuySellFlowState::ModeSelect { buy_or_sell: None },
                Some(country("NG")),
                2,
            ),
        ];

        for (name, step, detected_country, current) in &scenarios {
            let spanish =
                coincube_ui::locale::render_in(coincube_ui::locale::Language::Spanish, || {
                    BuySellPanel::entry_view(step, *detected_country)
                });
            capture::inspect(
                &format!("r19-buysell-{name}-es"),
                spanish,
                (1440, 900),
                "dark",
                theme::Theme::dark(),
            )
            .assert_spanish(&format!("Buy/Sell entry ({name})"));
            for (appearance, theme) in [
                ("dark", theme::Theme::dark()),
                ("light", theme::Theme::light()),
            ] {
                for size in [(1024, 768), (1440, 900)] {
                    let inspection = capture::inspect(
                        &format!("r19-buysell-{name}"),
                        BuySellPanel::entry_view(step, *detected_country),
                        size,
                        appearance,
                        theme,
                    );
                    assert!(has_exact(&inspection, "Buy/Sell"), "{}", name);
                    for (index, label) in ["Location", "Sign in", "Buy or sell"].iter().enumerate()
                    {
                        if index < *current {
                            assert!(has_exact(&inspection, label), "{}: {}", name, label);
                            assert!(
                                !has_exact(&inspection, &format!("{}  {label}", index + 1)),
                                "{}: completed step remains numbered",
                                name
                            );
                        } else {
                            let expected = format!("{}  {label}", index + 1);
                            assert!(has_exact(&inspection, &expected), "{}: {}", name, expected);
                        }
                    }
                    assert_eq!(
                        inspection
                            .text
                            .iter()
                            .filter(|(value, _)| value == "\u{f633}")
                            .count(),
                        *current,
                        "{name}: stage completion must come from the current state"
                    );
                    match *name {
                        "detecting" => assert!(inspection.contains("Detecting your location...")),
                        "manual-country" => assert!(inspection.contains("Select Country:")),
                        "login" => assert!(inspection.contains("Sign in to your account")),
                        "otp" => assert!(inspection.contains("Resend in 28s")),
                        "mode" => {
                            assert!(inspection.contains("View Order History"));
                            assert!(inspection.contains("Log Out"));
                        }
                        _ => unreachable!(),
                    }
                    if *name == "mode" {
                        assert!(inspection.contains("Mavapay"));
                    }
                    if *name == "manual-country" || *name == "detecting" {
                        assert!(!inspection.contains("Provider"));
                    }
                }
            }
        }
    }
}

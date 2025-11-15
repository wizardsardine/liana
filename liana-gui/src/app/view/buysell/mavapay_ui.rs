use crate::app::view::{
    buysell::{flow_state::MavapayFlowMode, MavapayFlowStep, MavapayState},
    BuySellMessage, MavapayMessage,
};

use iced::{widget::*, Alignment, Length};

use liana_ui::component::{button, form, text};
use liana_ui::{color, theme, widget::Column};

pub fn form<'a>(state: &'a MavapayState) -> Column<'a, BuySellMessage> {
    match &state.step {
        MavapayFlowStep::Login { password, email } => login_form(password, email),
        MavapayFlowStep::Register { .. } => registration_form(state),
        MavapayFlowStep::VerifyEmail { .. } => email_verification_form(state),
        MavapayFlowStep::ActiveBuysell { .. } => active_form(state),
    }
}

fn login_form<'a>(password: &'a str, email: &'a str) -> Column<'a, BuySellMessage> {
    iced::widget::column![
        // header
        text::h3("Sign in to your account").color(color::WHITE),
        Space::with_height(Length::Fixed(30.0)),
        // input fields
        text_input("Email", password)
            .on_input(|e| { BuySellMessage::Mavapay(MavapayMessage::LoginUsernameChanged(e)) }),
        Space::with_height(Length::Fixed(20.0)),
        text_input("Password", email)
            .secure(true)
            .on_input(|p| { BuySellMessage::Mavapay(MavapayMessage::LoginPasswordChanged(p)) }),
        Space::with_height(Length::Fixed(30.0)),
        // submit button
        button::primary(None, "Sign In")
            .on_press_maybe(
                // TODO: better form validation?
                (!email.is_empty() && !email.is_empty())
                    .then_some(BuySellMessage::Mavapay(MavapayMessage::SubmitLogin)),
            )
            .width(Length::Fill),
        Space::with_height(Length::Fixed(20.0)),
        // sign-up redirect
        iced::widget::button(
            text::p2_regular("Don't have an account? Sign up").color(color::ORANGE),
        )
        .style(theme::button::transparent)
        .on_press(BuySellMessage::Mavapay(MavapayMessage::CreateNewAccount))
    ]
    .align_x(Alignment::Center)
    .spacing(5)
    .max_width(500)
    .width(Length::Fill)
}

fn registration_form<'a>(state: &'a MavapayState) -> Column<'a, BuySellMessage> {
    use liana_ui::icon::previous_icon;

    let MavapayFlowStep::Register {
        first_name,
        last_name,
        password1,
        password2,
        email,
    } = &state.step
    else {
        unreachable!();
    };

    iced::widget::column![
        // Top bar with previous
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
        Space::with_height(Length::Fixed(10.0)),
        // Title and subtitle
        iced::widget::column![
            text::h3("Create an Account").color(color::WHITE),
            text::p2_regular("Get started with your personal Bitcoin wallet. Buy, store, and manage crypto securely, all in one place.").color(color::GREY_3)
        ]
        .spacing(10)
        .align_x(Alignment::Center),
        Space::with_height(Length::Fixed(20.0)),
        // Name Inputs
        iced::widget::row![
            Container::new(
                form::Form::new("First Name", first_name, |v| {
                    BuySellMessage::Mavapay(MavapayMessage::FirstNameChanged(v))
                })
                .size(16)
                .padding(15),
            )
            .width(Length::FillPortion(1)),
            Space::with_width(Length::Fixed(12.0)),
            Container::new(
                form::Form::new("Last Name", last_name, |v| {
                    BuySellMessage::Mavapay(MavapayMessage::LastNameChanged(v))
                })
                .size(16)
                .padding(15),
            )
            .width(Length::FillPortion(1))
        ],
        Space::with_height(Length::Fixed(10.0)),
        // Email Input
        form::Form::new("Email Address", email, |v| {
            BuySellMessage::Mavapay(MavapayMessage::EmailChanged(v))
        })
        .size(16)
        .padding(15),
        Space::with_height(Length::Fixed(10.0)),
        // Password Inputs
        form::Form::new("Password", password1, |v| {
            BuySellMessage::Mavapay(MavapayMessage::Password1Changed(v))
        })
        .size(16)
        .padding(15)
        .secure(),
        // TODO: include password check messages
        form::Form::new("Confirm Password", password2, |v| {
            BuySellMessage::Mavapay(MavapayMessage::Password2Changed(v))
        })
        .size(16)
        .padding(15)
        .secure(),
        Space::with_height(Length::Fixed(20.0)),
        button::primary(None, "Create Account")
            .on_press_maybe(
                {
                    // TODO: implement form validation here
                    // use https://github.com/shssoichiro/zxcvbn-rs for password strength validation
                    false
                }
                .then_some(BuySellMessage::Mavapay(MavapayMessage::SubmitRegistration)),
            )
            .width(Length::Fill),
    ]
    .align_x(Alignment::Center)
    .spacing(5)
    .max_width(500)
    .width(Length::Fill)
}

fn email_verification_form<'a>(state: &'a MavapayState) -> Column<'a, BuySellMessage> {
    use liana_ui::icon::{email_icon, previous_icon, reload_icon};

    let MavapayFlowStep::VerifyEmail {
        email, checking, ..
    } = &state.step
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
        false => {
            text::p2_regular("We need to verify your email before you continue").color(color::WHITE)
        }
    };

    // Email display
    let email_display = Column::new()
        .push(text::p2_regular(email).color(color::WHITE))
        .spacing(10)
        .align_x(Alignment::Center);

    // Action buttons
    let action_buttons = match checking {
        true => Row::new()
            .push(
                text::p1_italic("You'll be automatically logged in after verifying your email")
                    .width(Length::Fill),
            )
            .spacing(10),
        false => Row::new()
            .push(
                button::secondary(Some(reload_icon()), "Check Status")
                    .on_press(BuySellMessage::Mavapay(
                        MavapayMessage::CheckEmailVerificationStatus,
                    ))
                    .width(Length::FillPortion(1)),
            )
            .push(Space::with_width(Length::Fixed(10.0)))
            .push(
                button::primary(Some(email_icon()), "Resend Email").on_press(
                    BuySellMessage::Mavapay(MavapayMessage::SendVerificationEmail),
                ),
            )
            .spacing(10)
            .align_y(Alignment::Center),
    };

    iced::widget::column![
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
    .width(Length::Fill)
}

fn active_form<'a>(state: &'a MavapayState) -> Column<'a, BuySellMessage> {
    use liana_ui::icon::bitcoin_icon;

    let MavapayFlowStep::ActiveBuysell {
        flow_mode,
        amount,
        source_currency,
        target_currency,
        payment_method,
        bank_account_number,
        bank_account_name,
        bank_code,
        bank_name,
        current_quote,
        current_price,
        settlement_currency,
        ..
    } = &state.step
    else {
        unreachable!()
    };

    let header = Row::new()
        .push(Space::with_width(Length::Fill))
        .push(text::h4_bold("Bitcoin ↔ Fiat Exchange").color(color::WHITE))
        .push(Space::with_width(Length::Fill))
        .align_y(Alignment::Center);

    let mut column = Column::new()
        .push(header)
        .push(Space::with_height(Length::Fixed(20.0)));

    // Current price display
    if let Some(price) = current_price {
        column = column
            .push(
                Container::new(
                    Row::new()
                        .push(bitcoin_icon().size(20).color(color::ORANGE))
                        .push(Space::with_width(Length::Fixed(10.0)))
                        .push(
                            text(format!(
                                "1 BTC = {:.4} {}",
                                price.btc_price_in_unit_currency, price.currency
                            ))
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
                        Some(*flow_mode),
                        |mode| BuySellMessage::Mavapay(MavapayMessage::FlowModeChanged(mode)),
                    )
                    .size(16)
                    .text_size(14),
                )
                .push(Space::with_width(Length::Fixed(20.0)))
                .push(
                    radio(
                        "One-time Payment",
                        MavapayFlowMode::OneTimePayment,
                        Some(*flow_mode),
                        |mode| BuySellMessage::Mavapay(MavapayMessage::FlowModeChanged(mode)),
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
                iced_aw::number_input(amount, .., |a| {
                    BuySellMessage::Mavapay(MavapayMessage::AmountChanged(a))
                })
                .size(14)
                .padding(10),
            )
            .width(Length::Fixed(200.0)),
        )
        .push(Space::with_height(Length::Fixed(15.0)));

    // Conditional fields based on mode
    match flow_mode {
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
                                        crate::services::mavapay::MavapayUnitCurrency::all(),
                                        source_currency.as_ref(),
                                        |currency| {
                                            BuySellMessage::Mavapay(
                                                MavapayMessage::SourceCurrencyChanged(currency),
                                            )
                                        },
                                    )
                                    .placeholder("Select Source Currency")
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
                                        crate::services::mavapay::MavapayUnitCurrency::all(),
                                        target_currency.as_ref(),
                                        |currency| {
                                            BuySellMessage::Mavapay(
                                                MavapayMessage::TargetCurrencyChanged(currency),
                                            )
                                        },
                                    )
                                    .placeholder("Select Target Currency")
                                    .style(theme::pick_list::primary)
                                    .padding(10),
                                )
                                .width(Length::Fixed(250.0)),
                        )
                        .spacing(10),
                )
                .push(Space::with_height(Length::Fixed(15.0)));

            // Bank Account Details (only when selling BTC in Create Quote mode)
            if matches!(
                source_currency,
                Some(crate::services::mavapay::MavapayUnitCurrency::BitcoinSatoshi)
            ) {
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
                                        text_input("...1234567890", &bank_account_number)
                                            .on_input(|ban| {
                                                BuySellMessage::Mavapay(
                                                    MavapayMessage::BankAccountNumberChanged(ban),
                                                )
                                            })
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
                                        text_input("John/Jane Doe", &bank_account_name)
                                            .on_input(|ban| {
                                                BuySellMessage::Mavapay(
                                                    MavapayMessage::BankAccountNameChanged(ban),
                                                )
                                            })
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
                                        text_input("...", &bank_code)
                                            .on_input(|ban| {
                                                BuySellMessage::Mavapay(
                                                    MavapayMessage::BankCodeChanged(ban),
                                                )
                                            })
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
                                        text_input("...", &bank_name)
                                            .on_input(|ban| {
                                                BuySellMessage::Mavapay(
                                                    MavapayMessage::BankNameChanged(ban),
                                                )
                                            })
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
                        crate::services::mavapay::MavapayCurrency::all(),
                        settlement_currency.as_ref(),
                        |currency| {
                            BuySellMessage::Mavapay(MavapayMessage::SettlementCurrencyChanged(
                                currency,
                            ))
                        },
                    )
                    .placeholder("Select Settlement Currency")
                    .style(theme::pick_list::primary)
                    .padding(10)
                    .width(Length::Fixed(250.0)),
                )
                .push(Space::with_height(Length::Fixed(15.0)))
                .push(text("Payment Method").size(14).color(color::GREY_3))
                .push(Space::with_height(Length::Fixed(5.0)))
                .push(
                    pick_list(
                        crate::services::mavapay::MavapayPaymentMethod::all(),
                        Some(payment_method),
                        |method| {
                            BuySellMessage::Mavapay(MavapayMessage::PaymentMethodChanged(method))
                        },
                    )
                    .style(theme::pick_list::primary)
                    .padding(10)
                    .width(Length::Fixed(250.0)),
                )
                .push(Space::with_height(Length::Fixed(15.0)));
        }
    }

    // TODO: disable button if form is not valid
    form_column = form_column
        .push(
            button::primary(None, "Process Payment")
                .on_press(match flow_mode {
                    MavapayFlowMode::CreateQuote => {
                        BuySellMessage::Mavapay(MavapayMessage::CreateQuote)
                    }
                    MavapayFlowMode::OneTimePayment => {
                        BuySellMessage::Mavapay(MavapayMessage::OpenPaymentLink)
                    }
                })
                .width(Length::Fill),
        )
        .spacing(5);

    let exchange_form = Container::new(form_column)
        .padding(20)
        .style(theme::card::simple)
        .width(Length::Fixed(600.0)); // Fixed width for consistent layout

    column = column.push(exchange_form);

    // Quote display with payment confirmation
    if let Some(quote) = current_quote {
        let mut quote_column = Column::new()
            .push(text::h5_medium("Quote Created Successfully").color(color::GREEN))
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
            )
            .push(Space::with_height(Length::Fixed(10.0)))
            .push(text("Lightning Invoice: ").size(14).color(color::GREY_3))
            .push(
                Container::new(text(&quote.invoice).size(12).color(color::WHITE))
                    .padding(10)
                    .style(theme::card::simple),
            );

        // Show NGN bank details if available (for buy-BTC flow)
        if let Some(bank_name) = quote.bank_name.as_deref() {
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
                                text(format!("Bank: {}", bank_name))
                                    .size(12)
                                    .color(color::WHITE),
                            )
                            .push_maybe(quote.ngn_bank_account_number.as_deref().map(|ban| {
                                text(format!("Account Number: {}", ban))
                                    .size(12)
                                    .color(color::WHITE)
                            }))
                            .push_maybe(quote.ngn_account_name.as_deref().map(|an| {
                                text(format!("Account Name: {}", an))
                                    .size(12)
                                    .color(color::WHITE)
                            }))
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

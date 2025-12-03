use crate::app::view::{
    buysell::{panel::BuyOrSell, MavapayFlowStep, MavapayState},
    BuySellMessage, MavapayMessage,
};

use iced::{widget::*, Alignment, Length};

use liana_ui::{color, theme, widget::Column};
use liana_ui::{
    component::{button, text},
    icon::*,
};

// TODO: Use labels instead of placeholders for all input forms
pub fn form<'a>(state: &'a MavapayState) -> Column<'a, BuySellMessage> {
    let form = match &state.step {
        MavapayFlowStep::Register { .. } => registration_form,
        MavapayFlowStep::VerifyEmail { .. } => email_verification_form,
        MavapayFlowStep::Login { .. } => login_form,
        MavapayFlowStep::PasswordReset { .. } => password_reset_form,
        MavapayFlowStep::Transaction { .. } => active_form,
    };

    form(state)
}

fn login_form<'a>(state: &'a MavapayState) -> Column<'a, BuySellMessage> {
    let MavapayFlowStep::Login { email, password } = &state.step else {
        unreachable!();
    };

    iced::widget::column![
        // header
        text::h3("Sign in to your account").color(color::WHITE),
        Space::with_height(Length::Fixed(35.0)),
        // input fields
        text_input("Email", email)
            .on_input(|e| BuySellMessage::Mavapay(MavapayMessage::LoginUsernameChanged(e)))
            .size(16)
            .padding(15),
        Space::with_height(Length::Fixed(5.0)),
        text_input("Password", password)
            .secure(true)
            .on_input(|p| BuySellMessage::Mavapay(MavapayMessage::LoginPasswordChanged(p)))
            .size(16)
            .padding(15),
        Space::with_height(Length::Fixed(15.0)),
        // submit button
        button::primary(None, "Log In")
            .on_press_maybe(
                (email.contains('.') & email.contains('@') && !password.is_empty()).then_some(
                    BuySellMessage::Mavapay(MavapayMessage::SubmitLogin {
                        skip_email_verification: false
                    })
                ),
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
        .on_press(BuySellMessage::Mavapay(MavapayMessage::CreateNewAccount)),
        // password reset button
        iced::widget::button(
            text::p2_regular("Forgot your Password? Reset it here...").color(color::ORANGE),
        )
        .style(theme::button::link)
        .on_press(BuySellMessage::Mavapay(MavapayMessage::ResetPassword))
    ]
    .align_x(Alignment::Center)
    .spacing(2)
    .max_width(500)
    .width(Length::Fill)
}

fn password_reset_form<'a>(state: &'a MavapayState) -> Column<'a, BuySellMessage> {
    let MavapayFlowStep::PasswordReset { email, sent } = &state.step else {
        unreachable!()
    };

    iced::widget::column![
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
                        .on_input(|s| BuySellMessage::Mavapay(MavapayMessage::EmailChanged(s)))
                        .on_submit(BuySellMessage::Mavapay(
                            MavapayMessage::SendPasswordResetEmail,
                        ))
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
                    text::p2_regular("Recovery Email Sent! Return to Log-In").color(color::BLUE),
                )
                .style(theme::button::link)
                .on_press(BuySellMessage::Mavapay(MavapayMessage::ReturnToLogin)),
                // sends the password reset email
                false => button(text::p2_medium("Proceed").size(16).width(80).center())
                    .on_press_maybe(
                        (!*sent && email.contains('.') && email.contains('@')).then_some(
                            BuySellMessage::Mavapay(MavapayMessage::SendPasswordResetEmail)
                        )
                    ),
            },
        ]
        .align_x(iced::Alignment::Center),
    )
    .align_x(iced::Alignment::Center)
    .spacing(2)
    .max_width(400)
    .width(Length::Fill)
}

fn registration_form<'a>(state: &'a MavapayState) -> Column<'a, BuySellMessage> {
    use liana_ui::icon::previous_icon;

    let MavapayFlowStep::Register {
        legal_name,
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
        text_input("Full Legal Name: ", legal_name).on_input(|v| BuySellMessage::Mavapay(MavapayMessage::LegalNameChanged(v)))
            .width(Length::Fill)
            .size(16)
            .padding(15),
        // Email Input
        text_input("Email Address", email).on_input(|v| {
            BuySellMessage::Mavapay(MavapayMessage::EmailChanged(v))
        })
        .size(16)
        .padding(15),
        Space::with_height(Length::Fixed(10.0)),
        // Password Inputs
        text_input("Password", password1).on_input(|v| {
            BuySellMessage::Mavapay(MavapayMessage::Password1Changed(v))
        })
        .size(16)
        .padding(15)
        .secure(true),
        // TODO: include password check messages
        text_input("Confirm Password", password2).on_input(|v| {
            BuySellMessage::Mavapay(MavapayMessage::Password2Changed(v))
        })
        .size(16)
        .padding(15)
        .secure(true),
        Space::with_height(Length::Fixed(20.0)),
        button::primary(None, "Create Account")
            .on_press_maybe(
                (!legal_name.is_empty() && email.contains('.') &&  email.contains('@')  && !password1.is_empty() && (password1 == password2))
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

    let MavapayFlowStep::Transaction {
        amount,
        current_quote,
        current_price,
        buy_or_sell,
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
                        .push(
                            text(format!(
                                "1 SAT = {:.4} {}",
                                price.btc_price_in_unit_currency / 100_000_000.0,
                                price.currency
                            ))
                            .size(16)
                            .color(color::WHITE),
                        )
                        .push(Space::with_width(Length::Fill))
                        .push(bitcoin_icon().size(20).color(color::ORANGE))
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
        // Amount field (common to both modes)
        .push(text("Amount in BTCSAT").size(14).color(color::GREY_3))
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
        .push(Space::with_height(Length::Fixed(15.0)))
        // TODO: Display source/target currencies, with realtime conversion rate
        .push(text("unimplemented!"))
        .push(Space::with_height(Length::Fixed(15.0)));

    match buy_or_sell {
        BuyOrSell::Buy { address: _ } => {
            // TODO: display input amount, generated address and bank deposit details.
        }
        BuyOrSell::Sell => {
            // TODO: display onchain bitcoin address for deposit, and beneficiary input forms
        }
    }

    // TODO: disable button if form is not valid (the mavapay API has minimum amounts specified in the documentation)
    form_column = form_column
        .push(
            button::primary(None, "Process Payment")
                .on_press(BuySellMessage::Mavapay(MavapayMessage::CreateQuote))
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

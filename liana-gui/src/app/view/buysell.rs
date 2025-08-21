use iced::{
    widget::{container, text, Space, checkbox, Button, button},
    Alignment, Length,
};

use liana_ui::{
    color,
    component::{button as ui_button, text::*, modal::Modal, form},
    icon::{ bitcoin_icon, building_icon, person_icon },
    theme,
    widget::*,
};

use crate::app::{
    state::buysell::{AccountType, BuyAndSellPanel, BuySellStep},
    view::{BuySellMessage, Message as ViewMessage},
};

pub fn buysell_view(state: &BuyAndSellPanel) -> Element<'_, ViewMessage> {
    // Create the base content (what would be shown when modal is not active)
    let base_content = buysell_base_content();

    // If modal should be shown, overlay it on the base content
    if state.show_modal {
        Modal::new(
            base_content,
            buysell_modal_content(state)
        )
        .on_blur(Some(ViewMessage::BuySell(BuySellMessage::CloseModal)))
        .into()
    } else {
        base_content
    }
}

fn buysell_base_content() -> Element<'static, ViewMessage> {
    // This is the content that shows when the modal is not active
    // For now, show a simple placeholder that indicates buy/sell functionality
    Container::new(
        Column::new()
            .push(
                text("Buy & Sell Bitcoin")
                    .size(H2_SIZE)
                    .color(color::WHITE)
            )
            .push(Space::with_height(Length::Fixed(20.0)))
            .push(
                text("Connect to services to buy and sell Bitcoin directly from your Liana wallet.")
                    .size(16)
                    .color(color::GREY_3)
            )
            .push(Space::with_height(Length::Fixed(40.0)))
            .push(
                ui_button::primary(None, "Get Started")
                    .on_press(ViewMessage::BuySell(BuySellMessage::ShowModal))
                    .width(Length::Fixed(200.0))
            )
            .align_x(Alignment::Center)
            .spacing(10)
            .max_width(600)
    )
    .center_x(Length::Fill)
    .center_y(Length::Fill)
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

fn buysell_modal_content(state: &BuyAndSellPanel) -> Element<'_, ViewMessage> {
    match state.current_step {
        BuySellStep::Initial | BuySellStep::AccountSelection => {
            account_selection_modal_content(state)
        }
        BuySellStep::AccountForm => {
            account_form_modal_content(state)
        }
    }
}



fn account_selection_modal_content(state: &BuyAndSellPanel) -> Element<'_, ViewMessage> {
    let individual_selected = matches!(state.selected_account_type, Some(AccountType::Individual));
    let business_selected = matches!(state.selected_account_type, Some(AccountType::Business));
    let can_get_started = state.selected_account_type.is_some();

    Container::new(
        Column::new()
            .push(
                Row::new()
                    .push(
                        ui_button::transparent(None, "← Previous")
                            .on_press(ViewMessage::BuySell(BuySellMessage::CloseModal)),
                    )
                    .push(Space::with_width(Length::Fill))
                    .align_y(Alignment::Center)
                    .padding([20, 20]),
            )
            .push (
                Container::new(
                    Column::new()
                        .push(
                            Row::new()
                                .push(
                                    Column::new()
                                        .push(text("COINCUBE").size(16).color(color::ORANGE))
                                        .push(text("BUY/SELL").size(14).color(color::GREY_3))
                                        .spacing(2)
                                )
                                .align_y(Alignment::Center)
                                .padding([0, 20]),
                        )
                        .push(
                            text("Choose your account type")
                                .size(H2_SIZE)
                                .color(color::WHITE)
                        )
                        .push(Space::with_height(Length::Fixed(40.0)))
                        .push(
                            // Individual
                            Button::new(
                                Row::new()
                                    .push(
                                        Container::new(person_icon())
                                            .padding(15)
                                            .style(|_| container::Style {
                                                background: Some(iced::Background::Color(color::GREY_6)),
                                                border: iced::Border::default().rounded(5),
                                                ..Default::default()
                                            })
                                    )
                                    .push(Space::with_width(Length::Fixed(20.0)))
                                    .push(
                                        Column::new()
                                            .push(text("Individual").size(18).color(color::ORANGE))
                                            .push(text("For individuals who want to buy and manage Bitcoin").size(14).color(color::GREY_3))
                                            .spacing(5)
                                    )
                                    .align_y(Alignment::Center)
                                    .padding(20)
                            )
                            .style(move |_, _| button::Style {
                                background: Some(iced::Background::Color(
                                    if individual_selected {
                                        color::GREY_5
                                    } else {
                                        color::GREY_6
                                    }
                                )),
                                border: iced::Border{
                                    color: if individual_selected {
                                        color::ORANGE
                                    } else {
                                        color::GREY_4
                                    },
                                    width: if individual_selected {
                                        2.0
                                    } else {
                                        1.0
                                    },
                                    radius: 10.0.into(),
                                },
                                ..Default::default()
                            })
                            .on_press(ViewMessage::BuySell(
                                BuySellMessage::SelectAccountType(AccountType::Individual)
                            ))
                            .width(Length::Fill)
                        )
                        .push(Space::with_height(Length::Fixed(20.0)))
                        .push(
                            // Business
                            Button::new(
                                Row::new()
                                    .push(
                                        Container::new(building_icon())
                                            .padding(15)
                                            .style(|_| container::Style {
                                                background: Some(iced::Background::Color(color::GREY_6)),
                                                border: iced::Border::default().rounded(5),
                                                ..Default::default()
                                            })
                                    )
                                    .push(Space::with_width(Length::Fixed(20.0)))
                                    .push(
                                        Column::new()
                                            .push(text("Business").size(18).color(color::ORANGE))
                                            .push(text("For LLCs, trusts, corporations, partnerships, and more who want to buy and manage Bitcoin.").size(14).color(color::GREY_3))
                                            .spacing(5)
                                    )
                                    .align_y(Alignment::Center)
                                    .padding(20)
                            )
                            .style(move |_, _| button::Style {
                                background: Some(iced::Background::Color(
                                    if business_selected {
                                        color::GREY_5
                                    } else {
                                        color::GREY_6
                                    }
                                )),
                                border: iced::Border{
                                    color: if business_selected {
                                        color::ORANGE
                                    } else {
                                        color::GREY_4
                                    },
                                    width: if business_selected {
                                        2.0
                                    } else {
                                        1.0
                                    },
                                    radius: 10.0.into(),
                                },
                                ..Default::default()
                            })
                            .on_press(ViewMessage::BuySell(
                                BuySellMessage::SelectAccountType(AccountType::Business)
                            ))
                            .width(Length::Fill)
                        )
                        .push(Space::with_height(Length::Fixed(40.0)))
                        .push(
                            Container::new(
                                if can_get_started {
                                    ui_button::primary(None, "Get Started")
                                        .on_press(ViewMessage::BuySell(
                                            BuySellMessage::GetStarted
                                        ))
                                        .width(Length::Fixed(200.0))
                                } else {
                                    ui_button::secondary(None, "Get Started")
                                        .width(Length::Fixed(200.0))
                                }
                            )
                            .center_x(Length::Fill)
                        )
                        .align_x(Alignment::Center)
                        .spacing(10)
                        .max_width(600)
                        .width(Length::Fill)
                )
                .padding(40)
                .center_x(Length::Fill)
            )
            .spacing(10)
    )
    .style(theme::card::modal)
    .padding(40)
    .max_width(700)
    .into()
}

fn account_form_modal_content(state: &BuyAndSellPanel) -> Element<'_, ViewMessage> {
    let form_valid = state.is_form_valid();

    Container::new(
        Column::new()
            .push(
                Row::new()
                    .push(
                        ui_button::transparent(None, "← Previous")
                            .on_press(ViewMessage::BuySell(BuySellMessage::GoBack)),
                    )
                    .push(Space::with_width(Length::Fill))
                    .align_y(Alignment::Center)
                    .padding([10, 0]),
            )
            .push(
                Container::new(
                    Column::new()
                        .push(
                            Row::new()
                                .push(bitcoin_icon().size(32))
                                .push(Space::with_width(Length::Fixed(15.0)))
                                .push(
                                    Column::new()
                                        .push(text("COINCUBE").size(16).color(color::ORANGE))
                                        .push(text("BUY/SELL").size(14).color(color::GREY_3))
                                        .spacing(2)
                                )
                                .align_y(Alignment::Center)
                                .padding([30, 30]),
                        )
                        .push(
                            text("Create an Account")
                                .size(H2_SIZE)
                                .color(color::WHITE)
                        )
                        .push(Space::with_height(Length::Fixed(10.0)))
                        .push(
                            text("Get started with your personal Bitcoin wallet. Buy, store, and manage crypto securely, all in one place.")
                                .size(14)
                                .color(color::GREY_3)
                        )
                        .push(Space::with_height(Length::Fixed(30.0)))
                        .push(
                            // Continue with Google button (placeholder)
                            ui_button::secondary(None, "🔗 Continue with Google")
                                .width(Length::Fill)
                        )
                        .push(Space::with_height(Length::Fixed(20.0)))
                        .push(
                            Row::new()
                                .push(
                                    Container::new(
                                        Row::new()
                                            .push(Space::with_width(Length::Fill))
                                            .push(text("Or").size(14).color(color::GREY_3))
                                            .push(Space::with_width(Length::Fill))
                                    )
                                    .width(Length::Fill)
                                    .style(|_| container::Style {
                                        border: iced::Border {
                                            color: color::GREY_4,
                                            width: 1.0,
                                            radius: 0.0.into(),
                                        },
                                        ..Default::default()
                                    })
                                    .padding(10)
                                )
                        )
                        .push(Space::with_height(Length::Fixed(20.0)))
                        .push(
                            // Form fields
                            Row::new()
                                .push(
                                    form::Form::new_trimmed("First Name", &state.form_data.first_name, |value| {
                                        ViewMessage::BuySell(BuySellMessage::FormFieldEdited("first_name".to_string(), value))
                                    })
                                    .maybe_warning(if state.form_data.first_name.valid { None } else { Some("First name is required") })
                                    .size(P1_SIZE)
                                    .padding(10)
                                )
                                .push(Space::with_width(Length::Fixed(10.0)))
                                .push(
                                    form::Form::new_trimmed("Last Name", &state.form_data.last_name, |value| {
                                        ViewMessage::BuySell(BuySellMessage::FormFieldEdited("last_name".to_string(), value))
                                    })
                                    .maybe_warning(if state.form_data.last_name.valid { None } else { Some("Last name is required") })
                                    .size(P1_SIZE)
                                    .padding(10)
                                )
                                .spacing(10)
                        )
                        .push(Space::with_height(Length::Fixed(10.0)))
                        .push(
                            form::Form::new_trimmed("Email Address", &state.form_data.email, |value| {
                                ViewMessage::BuySell(BuySellMessage::FormFieldEdited("email".to_string(), value))
                            })
                            .maybe_warning(if state.form_data.email.valid { None } else { Some("Please enter a valid email address") })
                            .size(P1_SIZE)
                            .padding(10)
                        )
                        .push(Space::with_height(Length::Fixed(10.0)))
                        .push(
                            form::Form::new("Password", &state.form_data.password, |value| {
                                ViewMessage::BuySell(BuySellMessage::FormFieldEdited("password".to_string(), value))
                            })
                            .maybe_warning(if state.form_data.password.valid { None } else { Some("Password must be at least 8 characters") })
                            .size(P1_SIZE)
                            .padding(10)
                            .secure()
                        )
                        .push(Space::with_height(Length::Fixed(10.0)))
                        .push(
                            form::Form::new("Confirm Password", &state.form_data.confirm_password, |value| {
                                ViewMessage::BuySell(BuySellMessage::FormFieldEdited("confirm_password".to_string(), value))
                            })
                            .maybe_warning(if state.form_data.confirm_password.valid { None } else { Some("Passwords do not match") })
                            .size(P1_SIZE)
                            .padding(10)
                            .secure()
                        )
                        .push(Space::with_height(Length::Fixed(20.0)))
                        .push(
                            Row::new()
                                .push(
                                    checkbox("", state.form_data.terms_accepted)
                                        .on_toggle(|_| ViewMessage::BuySell(BuySellMessage::ToggleTermsAcceptance))
                                )
                                .push(Space::with_width(Length::Fixed(10.0)))
                                .push(
                                    text("I agree to COINCUBE's Terms of Service and Privacy Policy")
                                        .size(12)
                                        .color(color::GREY_3)
                                )
                                .align_y(Alignment::Center)
                        )
                        .push(Space::with_height(Length::Fixed(30.0)))
                        .push(
                            if form_valid {
                                ui_button::primary(None, "Create Account")
                                    .on_press(ViewMessage::BuySell(BuySellMessage::CreateAccount))
                                    .width(Length::Fill)
                            } else {
                                ui_button::secondary(None, "Create Account")
                                    .width(Length::Fill)
                            }
                        )
                        .push(Space::with_height(Length::Fixed(20.0)))
                        .push(
                            Row::new()
                                .push(Space::with_width(Length::Fill))
                                .push(text("Already have a COINCUBE account? ").size(12).color(color::GREY_3))
                                .push(
                                    ui_button::transparent(None, "Log in")
                                )
                                .push(Space::with_width(Length::Fill))
                                .align_y(Alignment::Center)
                        )
                        .align_x(Alignment::Center)
                        .spacing(5)
                        .max_width(500)
                        .width(Length::Fill)
                )
                .padding(40)
                .center_x(Length::Fill)
            )
            .spacing(10)
    )
    .style(theme::card::modal)
    .padding(20)
    .max_width(600)
    .into()
}
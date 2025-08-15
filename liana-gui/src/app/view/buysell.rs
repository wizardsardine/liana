use iced::{
    widget::{button, column, container, row, text, Button, Space},
    Alignment, Length,
};

use liana_ui::{
    color,
    component::{button as ui_button, text::*, modal::Modal},
    icon::{ building_icon, person_icon },
    theme,
    widget::*,
};

use crate::app::{
    message::Message,
    state::buysell::{AccountType, BuyAndSellPanel},
    view::{BuySellMessage, Message as ViewMessage},
};

pub fn buysell_view(state: &BuyAndSellPanel) -> Element<ViewMessage> {
    let content = Container::new(
        Column::new()
            .push(
                Container::new(
                    Column::new()
                        .push(
                            Container::new(text("Buy & Sell Bitcoin").size(H3_SIZE)).padding(20).center_x(Length::Fill),
                        )
                        .push(
                            Container::new(text("Connect to CoinCube to buy and sell Bitcoin directly from your Liana wallet."))
                                .padding(20)
                                .center_x(Length::Fill),
                        )
                        .push(
                            Container::new(
                                ui_button::primary(None, "Get Started")
                                    .on_press(ViewMessage::BuySell(BuySellMessage::ShowAccountSelection))
                                    .width(Length::Fixed(200.0)),
                            )
                            .padding(20)
                            .center_x(Length::Fill),
                        ),
                )
                .style(theme::card::simple)
                .padding(40),
            )
    .align_x(Alignment::Center)
    .spacing(20)
    .max_width(600),
    )
    .center_x(Length::Fill)
    .center_y(Length::Fill);

    if state.show_account_selection {
        Modal::new(content, account_selection_modal(state))
            .on_blur(Some(ViewMessage::BuySell(
                BuySellMessage::HideAccountSelection
            )))
            .into()
    } else {
        content.into()
    }
}

fn account_selection_modal(state: &BuyAndSellPanel) -> Element<ViewMessage> {
    let individual_selected = matches!(state.selected_account_type, Some(AccountType::Individual));
    let business_selected = matches!(state.selected_account_type, Some(AccountType::Business));
    let can_get_started = state.selected_account_type.is_some();
    
    Container::new(
        Column::new()
            .push(
                Row::new()
                    .push(
                        ui_button::transparent(None, "← Back")
                            .on_press(ViewMessage::BuySell(
                                BuySellMessage::HideAccountSelection
                            )),
                    )
                    .push(Space::with_width(Length::Fill))
                    .align_y(Alignment::Center)
                    .padding([10, 0]),
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
                            // Get Started
                            Container::new(
                                if can_get_started {
                                    ui_button::primary(None, "Get Started")
                                        .on_press(ViewMessage::BuySell(
                                            BuySellMessage::GetStarted
                                        ))
                                } else {
                                    ui_button::secondary(None, "Get Started")
                                        .width(Length::Fill)
                                }
                            )
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
    .padding(20)
    .max_width(700)
    .width(Length::Fill)
    .into()
}
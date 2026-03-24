use breez_sdk_liquid::{bitcoin::Denomination, model::PreparePayOnchainResponse};
use coincube_ui::{
    color,
    component::{amount::*, button, form, text::*},
    icon::{
        arrow_down_up_icon, arrow_right, check_circle_icon, eye_outline_icon, eye_slash_icon,
        lightning_icon, usd_icon, vault_icon, warning_icon,
    },
    theme,
    widget::*,
};
use iced::{
    widget::{mouse_area, Button, Column, Space, Stack},
    Alignment, Length,
};
use iced_anim::AnimationBuilder;

use crate::app::breez::assets::format_usdt_display;
use crate::app::{
    menu::Menu,
    view::{vault::receive::address_card, FiatAmountConverter},
};
use crate::app::{
    menu::{LiquidSubMenu, UsdtSubMenu, VaultSubMenu},
    view::message::{HomeMessage, Message},
};
use coincube_core::miniscript::bitcoin::Amount;

#[derive(Clone, Copy, Debug)]
enum WalletType {
    Liquid,
    Usdt { balance: u64, error: bool },
    Vault,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IncomingTransferStage {
    TransferInitiated,
    SwappingLbtcToBtc,
    SendingToVault,
    Completed,
}

#[derive(Clone, Copy, Debug)]
pub struct PendingIncomingTransfer {
    pub amount: Amount,
    pub stage: IncomingTransferStage,
}

fn incoming_transfer_status_text(stage: IncomingTransferStage) -> &'static str {
    match stage {
        IncomingTransferStage::TransferInitiated => "Transfer initiated",
        IncomingTransferStage::SwappingLbtcToBtc => "Swapping LBTC to BTC",
        IncomingTransferStage::SendingToVault => "Sending BTC to Vault",
        IncomingTransferStage::Completed => "Completed",
    }
}

fn vault_incoming_transfer_card<'a>(
    pending_transfer: PendingIncomingTransfer,
    bitcoin_unit: BitcoinDisplayUnit,
    animation_phase: f32,
) -> Element<'a, Message> {
    let steps = [
        IncomingTransferStage::TransferInitiated,
        IncomingTransferStage::SwappingLbtcToBtc,
        IncomingTransferStage::SendingToVault,
        IncomingTransferStage::Completed,
    ];
    let current_step = steps
        .iter()
        .position(|stage| *stage == pending_transfer.stage)
        .unwrap_or(0);
    let step_labels = ["Initiated", "Swapped", "Sending", "Complete"];

    let step_dot = |idx: usize, current: usize| -> Element<'a, Message> {
        let color = if idx < current {
            color::GREEN
        } else if idx == current {
            color::ORANGE
        } else {
            color::GREY_4
        };
        Container::new(
            Space::new()
                .width(Length::Fixed(8.0))
                .height(Length::Fixed(8.0)),
        )
        .style(move |_| iced::widget::container::Style {
            background: Some(iced::Background::Color(color)),
            border: iced::Border {
                color,
                width: 1.0,
                radius: 20.0.into(),
            },
            ..Default::default()
        })
        .into()
    };

    let step_line = |segment_idx: usize, current: usize, phase: f32| -> Element<'a, Message> {
        let animate_segment = if current == 0 { 0 } else { current - 1 };
        if segment_idx == animate_segment && current < 4 {
            use iced_anim::spring::Motion;
            return AnimationBuilder::new(phase, move |animated_phase| {
                let mut wave = Row::new().spacing(2).align_y(Alignment::Center);

                for i in 0..4 {
                    let shifted = (animated_phase + i as f32 * 0.18) % 1.0;
                    let intensity = 1.0 - ((shifted * 2.0 - 1.0).abs());
                    let alpha = 0.25 + 0.75 * intensity;

                    wave = wave.push(
                        Container::new(
                            Space::new()
                                .width(Length::Fixed(6.0))
                                .height(Length::Fixed(2.0)),
                        )
                        .style(move |_| iced::widget::container::Style {
                            background: Some(iced::Background::Color(iced::Color {
                                a: alpha,
                                ..color::ORANGE
                            })),
                            ..Default::default()
                        }),
                    );
                }

                wave.into()
            })
            .animation(Motion::SMOOTH)
            .animates_layout(false)
            .into();
        }

        if segment_idx < current {
            return Container::new(
                Space::new()
                    .width(Length::Fixed(28.0))
                    .height(Length::Fixed(1.0)),
            )
            .style(|_| iced::widget::container::Style {
                background: Some(iced::Background::Color(color::ORANGE)),
                ..Default::default()
            })
            .into();
        }

        Container::new(
            Space::new()
                .width(Length::Fixed(28.0))
                .height(Length::Fixed(1.0)),
        )
        .style(|_| iced::widget::container::Style {
            background: Some(iced::Background::Color(color::GREY_5)),
            ..Default::default()
        })
        .into()
    };

    let completed_chip = |label: &'static str| -> Element<'a, Message> {
        Container::new(
            Row::new()
                .spacing(5)
                .align_y(Alignment::Center)
                .push(check_circle_icon().size(10).color(color::GREEN))
                .push(text(label).size(10).color(color::GREY_2)),
        )
        .padding([4, 8])
        .style(|_| iced::widget::container::Style {
            background: Some(iced::Background::Color(color::GREY_6)),
            border: iced::Border {
                color: color::GREY_5,
                width: 0.5,
                radius: 12.0.into(),
            },
            ..Default::default()
        })
        .into()
    };

    let completed_steps_view: Element<'a, Message> = if current_step == 0 {
        Container::new(text("Starting...").size(10).color(color::GREY_3))
            .width(Length::Fill)
            .into()
    } else {
        step_labels[..current_step]
            .iter()
            .fold(Row::new().spacing(6), |row, label| {
                row.push(completed_chip(label))
            })
            .into()
    };

    Container::new(
        Column::new()
            .spacing(12)
            .push(
                Row::new()
                    .align_y(Alignment::Center)
                    .push(text("Incoming transfer").size(12).color(color::GREY_2))
                    .push(Space::new().width(Length::Fill))
                    .push(
                        text(
                            pending_transfer
                                .amount
                                .to_formatted_string_with_unit(bitcoin_unit),
                        )
                        .bold()
                        .size(14)
                        .color(color::ORANGE),
                    ),
            )
            .push(
                Row::new()
                    .spacing(8)
                    .align_y(Alignment::Center)
                    .push(step_dot(0, current_step))
                    .push(step_line(0, current_step, animation_phase))
                    .push(step_dot(1, current_step))
                    .push(step_line(1, current_step, animation_phase))
                    .push(step_dot(2, current_step))
                    .push(step_line(2, current_step, animation_phase))
                    .push(step_dot(3, current_step)),
            )
            .push(
                Row::new()
                    .align_y(Alignment::Center)
                    .push(text(incoming_transfer_status_text(pending_transfer.stage)).size(11))
                    .push(Space::new().width(Length::Fill))
                    .push(text("Liquid -> Vault").size(11).color(color::GREY_3)),
            )
            .push(completed_steps_view),
    )
    .padding([14, 16])
    .style(|_| iced::widget::container::Style {
        border: iced::Border {
            color: color::GREY_4,
            width: 0.5,
            radius: 20.0.into(),
        },
        background: Some(iced::Background::Color(color::GREY_6)),
        ..Default::default()
    })
    .into()
}

#[allow(clippy::too_many_arguments)]
fn wallet_card<'a>(
    wallet_type: WalletType,
    balance: &Amount,
    fiat_converter: Option<FiatAmountConverter>,
    balance_masked: bool,
    has_vault: bool,
    bitcoin_unit: coincube_ui::component::amount::BitcoinDisplayUnit,
    pending_vault_incoming: Option<PendingIncomingTransfer>,
    pending_animation_phase: f32,
    pending_send_sats: u64,
    pending_receive_sats: u64,
) -> Element<'a, Message> {
    let fiat_balance = fiat_converter.as_ref().map(|c| c.convert(*balance));

    let (icon, title, title_color, send_action, receive_action) = match wallet_type {
        WalletType::Liquid => (
            lightning_icon().color(color::ORANGE),
            "Liquid",
            Some(color::ORANGE),
            Message::Menu(Menu::Liquid(LiquidSubMenu::Send)),
            Message::Menu(Menu::Liquid(LiquidSubMenu::Receive)),
        ),
        WalletType::Usdt { .. } => (
            usd_icon().color(color::ORANGE),
            "USDt",
            Some(color::ORANGE),
            Message::Menu(Menu::Usdt(UsdtSubMenu::Send)),
            Message::Menu(Menu::Usdt(UsdtSubMenu::Receive)),
        ),
        WalletType::Vault => (
            vault_icon(),
            "Vault",
            None,
            Message::Menu(Menu::Vault(VaultSubMenu::Send)),
            Message::Menu(Menu::Vault(VaultSubMenu::Receive)),
        ),
    };

    // USDt card renders its own balance display (not Amount-based)
    if let WalletType::Usdt {
        balance: usdt_bal,
        error: usdt_error,
    } = wallet_type
    {
        let content = Column::new()
            .spacing(12)
            .push(
                Row::new()
                    .spacing(8)
                    .align_y(Alignment::Center)
                    .push(usd_icon().color(color::ORANGE).size(16))
                    .push(text("USDt").color(color::ORANGE).size(14)),
            )
            .push(
                Row::new()
                    .align_y(Alignment::Center)
                    .push(
                        Column::new()
                            .spacing(4)
                            .push(if balance_masked {
                                Row::new().push(text("********").size(H2_SIZE))
                            } else if usdt_error {
                                Row::new().push(
                                    text("Balance unavailable").size(P1_SIZE).color(color::RED),
                                )
                            } else {
                                Row::new()
                                    .spacing(10)
                                    .align_y(Alignment::Center)
                                    .push(text(format_usdt_display(usdt_bal)).size(H2_SIZE).bold())
                                    .push(text("USDt").size(H2_SIZE).color(color::GREY_3))
                            })
                            .push(
                                text("Liquid Network")
                                    .size(P1_SIZE)
                                    .style(theme::text::secondary),
                            )
                            .push_maybe(
                                (!balance_masked && !usdt_error && pending_send_sats > 0).then(
                                    || {
                                        Row::new()
                                            .spacing(6)
                                            .align_y(Alignment::Center)
                                            .push(
                                                warning_icon()
                                                    .size(12)
                                                    .style(theme::text::secondary),
                                            )
                                            .push(
                                                text(format!(
                                                    "-{} USDt pending",
                                                    format_usdt_display(pending_send_sats)
                                                ))
                                                .size(P2_SIZE)
                                                .style(theme::text::secondary),
                                            )
                                    },
                                ),
                            )
                            .push_maybe(
                                (!balance_masked && !usdt_error && pending_receive_sats > 0).then(
                                    || {
                                        Row::new()
                                            .spacing(6)
                                            .align_y(Alignment::Center)
                                            .push(
                                                warning_icon()
                                                    .size(12)
                                                    .style(theme::text::secondary),
                                            )
                                            .push(
                                                text(format!(
                                                    "+{} USDt pending",
                                                    format_usdt_display(pending_receive_sats)
                                                ))
                                                .size(P2_SIZE)
                                                .style(theme::text::secondary),
                                            )
                                    },
                                ),
                            )
                            .width(Length::Fill),
                    )
                    .push(Space::new().width(Length::Fixed(8.0)))
                    .push(
                        button::primary(None, "Send")
                            .width(Length::Fixed(120.0))
                            .on_press(Message::Menu(Menu::Usdt(UsdtSubMenu::Send))),
                    )
                    .push(Space::new().width(Length::Fixed(8.0)))
                    .push(
                        button::secondary(None, "Receive")
                            .style(|_t, _s| iced::widget::button::Style {
                                text_color: color::ORANGE,
                                border: iced::Border {
                                    color: color::ORANGE,
                                    width: 1.0,
                                    radius: 25.0.into(),
                                },
                                ..Default::default()
                            })
                            .width(Length::Fixed(120.0))
                            .on_press(Message::Menu(Menu::Usdt(UsdtSubMenu::Receive))),
                    ),
            );
        return Container::new(content)
            .padding(20)
            .style(|_| iced::widget::container::Style {
                border: iced::Border {
                    color: color::ORANGE,
                    width: 0.2,
                    radius: 25.0.into(),
                },
                background: Some(iced::Background::Color(color::GREY_6)),
                ..Default::default()
            })
            .into();
    }

    let content = match wallet_type {
        WalletType::Vault if !has_vault => Column::new().spacing(12).push(
            Row::new()
                .spacing(8)
                .align_y(Alignment::Center)
                .push(vault_icon())
                .push(text("Vault").size(14))
                .push(
                    Row::new()
                        .align_y(Alignment::Center)
                        .push(Space::new().width(Length::Fill))
                        .push(
                            button::primary(None, "Create Vault")
                                .width(Length::Fixed(160.0))
                                .on_press(Message::SetupVault),
                        ),
                ),
        ),
        _ => Column::new()
            .spacing(12)
            .push(
                Row::new()
                    .spacing(8)
                    .align_y(Alignment::Center)
                    .push(icon.size(16))
                    .push(
                        text(title)
                            .color(title_color.unwrap_or(color::GREY_2))
                            .size(14),
                    ),
            )
            .push(
                Row::new()
                    .align_y(Alignment::Center)
                    .push(
                        Column::new()
                            .spacing(4)
                            .push(if balance_masked {
                                Row::new().push(text("********").size(H2_SIZE))
                            } else {
                                amount_with_size_and_unit(balance, H2_SIZE, bitcoin_unit)
                            })
                            .push(if balance_masked {
                                Some(text("********").size(P1_SIZE))
                            } else {
                                fiat_balance
                                    .map(|fiat| fiat.to_text().size(P1_SIZE).color(color::GREY_2))
                            })
                            .push_maybe((!balance_masked && pending_send_sats > 0).then(|| {
                                Row::new()
                                    .spacing(6)
                                    .align_y(Alignment::Center)
                                    .push(warning_icon().size(12).style(theme::text::secondary))
                                    .push(text("-").size(P2_SIZE).style(theme::text::secondary))
                                    .push(amount_with_size_and_unit(
                                        &Amount::from_sat(pending_send_sats),
                                        P2_SIZE,
                                        bitcoin_unit,
                                    ))
                                    .push(
                                        text("pending").size(P2_SIZE).style(theme::text::secondary),
                                    )
                            }))
                            .push_maybe((!balance_masked && pending_receive_sats > 0).then(|| {
                                Row::new()
                                    .spacing(6)
                                    .align_y(Alignment::Center)
                                    .push(warning_icon().size(12).style(theme::text::secondary))
                                    .push(text("+").size(P2_SIZE).style(theme::text::secondary))
                                    .push(amount_with_size_and_unit(
                                        &Amount::from_sat(pending_receive_sats),
                                        P2_SIZE,
                                        bitcoin_unit,
                                    ))
                                    .push(
                                        text("pending").size(P2_SIZE).style(theme::text::secondary),
                                    )
                            }))
                            .width(Length::Fill),
                    )
                    .push(Space::new().width(Length::Fill))
                    .push_maybe(matches!(wallet_type, WalletType::Liquid).then(|| {
                        button::secondary(Some(arrow_down_up_icon()), "Transfer")
                            .style(|_t, _s| iced::widget::button::Style {
                                text_color: color::ORANGE,
                                border: iced::Border {
                                    color: color::ORANGE,
                                    width: 1.0,
                                    radius: 35.0.into(),
                                },
                                background: Some(iced::Background::Color(color::GREY_6)),
                                ..Default::default()
                            })
                            .width(Length::Fixed(140.0))
                            .on_press(Message::Home(HomeMessage::NextStep))
                    }))
                    .push_maybe(
                        matches!(wallet_type, WalletType::Liquid)
                            .then(|| Space::new().width(Length::Fixed(8.0))),
                    )
                    .push(
                        button::primary(None, "Send")
                            .width(Length::Fixed(120.0))
                            .on_press(send_action),
                    )
                    .push(Space::new().width(Length::Fixed(8.0)))
                    .push(
                        button::secondary(None, "Receive")
                            .style(|_t, _s| iced::widget::button::Style {
                                text_color: color::ORANGE,
                                border: iced::Border {
                                    color: color::ORANGE,
                                    width: 1.0,
                                    radius: 25.0.into(),
                                },
                                ..Default::default()
                            })
                            .width(Length::Fixed(120.0))
                            .on_press(receive_action),
                    ),
            ),
    };

    let content = if matches!(wallet_type, WalletType::Vault) {
        if let Some(pending_transfer) = pending_vault_incoming {
            if pending_transfer.stage != IncomingTransferStage::Completed {
                content.push(vault_incoming_transfer_card(
                    pending_transfer,
                    bitcoin_unit,
                    pending_animation_phase,
                ))
            } else {
                content
            }
        } else {
            content
        }
    } else {
        content
    };

    Container::new(content)
        .padding(20)
        .style(move |t| match wallet_type {
            WalletType::Liquid => iced::widget::container::Style {
                border: iced::Border {
                    color: color::ORANGE,
                    width: 0.2,
                    radius: 25.0.into(),
                },
                background: Some(iced::Background::Color(color::GREY_6)),
                ..Default::default()
            },
            WalletType::Usdt { .. } => iced::widget::container::Style {
                border: iced::Border {
                    color: color::ORANGE,
                    width: 0.2,
                    radius: 25.0.into(),
                },
                background: Some(iced::Background::Color(color::GREY_6)),
                ..Default::default()
            },
            WalletType::Vault => theme::card::simple(t),
        })
        .into()
}

fn transfer_direction_card<'a>(
    title: &'a str,
    description: &'a str,
    direction: TransferDirection,
    is_selected: bool,
) -> Element<'a, Message> {
    Container::new(
        Column::new().push(
            Button::new(
                Column::new()
                    .width(Length::Fill)
                    .align_x(Alignment::Center)
                    .push(
                        text(title)
                            .bold()
                            .style(theme::text::primary)
                            .size(P1_SIZE)
                            .align_x(Alignment::Center),
                    )
                    .push(
                        text(description)
                            .style(theme::text::secondary)
                            .align_x(Alignment::Center),
                    ),
            )
            .padding(20)
            .width(Length::Fill)
            .style(move |t, s| {
                if is_selected {
                    iced::widget::button::Style {
                        border: iced::Border {
                            color: color::ORANGE,
                            width: 1.0,
                            radius: 25.0.into(),
                        },
                        ..Default::default()
                    }
                } else {
                    theme::button::secondary(t, s)
                }
            })
            .on_press(Message::Home(HomeMessage::SelectTransferDirection(
                direction,
            ))),
        ),
    )
    .width(Length::Fill)
    .style(theme::card::simple)
    .into()
}

fn select_transfer_direction_view<'a>(
    direction: Option<TransferDirection>,
) -> Element<'a, Message> {
    let content =
        Column::new()
            .width(Length::Fill)
            .push(Space::new().height(Length::Fixed(60.0)))
            .push(
                button::secondary(None, "< Previous")
                    .width(Length::Fixed(150.0))
                    .on_press(Message::Home(HomeMessage::PreviousStep)),
            )
            .push(Space::new().height(Length::Fixed(20.0)))
            .push(
                Container::new(
                    Column::new()
                        .push(
                            Column::new()
                                .spacing(10)
                                .push(text("Transfer Between Wallets").bold().size(H2_SIZE))
                                .push(text("How do you want to move your funds?").size(P1_SIZE))
                                .align_x(Alignment::Center)
                                .width(Length::Fill),
                        )
                        .spacing(60)
                        .push(
                            Column::new()
                                .spacing(20)
                                .push(transfer_direction_card(
                                    "From Liquid to Vault",
                                    "Move funds into your secure Vault Wallet.",
                                    TransferDirection::LiquidToVault,
                                    matches!(direction, Some(TransferDirection::LiquidToVault)),
                                ))
                                .push(transfer_direction_card(
                                    "From Vault to Liquid",
                                    "Move funds back into your Liquid Wallet.",
                                    TransferDirection::VaultToLiquid,
                                    matches!(direction, Some(TransferDirection::VaultToLiquid)),
                                ))
                                .width(Length::Fill),
                        )
                        .push(button::primary(None, "Continue").on_press_maybe(
                            direction.map(|_dir| Message::Home(HomeMessage::NextStep)),
                        ))
                        .height(Length::Fixed(800.0))
                        .width(Length::Fixed(600.0))
                        .align_x(Alignment::Center),
                )
                .width(Length::Fill)
                .center_x(Length::Fill),
            );

    Container::new(content)
        .width(Length::Fill)
        .height(Length::Fixed(800.0))
        .center_y(Length::Fixed(800.0))
        .into()
}

fn balance_summary_card<'a>(
    wallet_name: &'a str,
    is_liquid: bool,
    balance: &Amount,
    fiat_converter: Option<FiatAmountConverter>,
    bitcoin_unit: crate::app::settings::unit::BitcoinDisplayUnit,
) -> Element<'a, Message> {
    let fiat_balance = fiat_converter.as_ref().map(|c| c.convert(*balance));

    let (icon, title_color) = if is_liquid {
        (lightning_icon().color(color::ORANGE), Some(color::ORANGE))
    } else {
        (vault_icon(), None)
    };

    let content = Column::new()
        .spacing(12)
        .push(
            Row::new()
                .spacing(8)
                .align_y(Alignment::Center)
                .push(icon.size(16))
                .push(
                    text(wallet_name)
                        .color(title_color.unwrap_or(color::GREY_2))
                        .size(14),
                ),
        )
        .push(
            Row::new().align_y(Alignment::Center).push(
                Column::new()
                    .spacing(4)
                    .push(amount_with_size_and_unit(balance, H2_SIZE, bitcoin_unit))
                    .push_maybe(
                        fiat_balance.map(|fiat| fiat.to_text().size(P1_SIZE).color(color::GREY_2)),
                    ),
            ),
        );

    Container::new(content)
        .padding(20)
        .width(Length::Fill)
        .style(move |t| {
            if is_liquid {
                iced::widget::container::Style {
                    border: iced::Border {
                        color: color::ORANGE,
                        width: 0.2,
                        radius: 25.0.into(),
                    },
                    background: Some(iced::Background::Color(color::GREY_6)),
                    ..Default::default()
                }
            } else {
                theme::card::simple(t)
            }
        })
        .into()
}

fn enter_amount_card<'a>(
    direction: TransferDirection,
    amount: &'a form::Value<String>,
    onchain_send_limit: Option<(u64, u64)>,
    onchain_receive_limit: Option<(u64, u64)>,
    bitcoin_unit: BitcoinDisplayUnit,
) -> Element<'a, Message> {
    let content = Column::new()
        .push(text("Enter Amount").bold().size(H2_SIZE))
        .push(Space::new().height(Length::Fixed(10.0)))
        .push(
            Row::new()
                .spacing(4)
                .push(text("Sending from"))
                .push(text(direction.display()).bold()),
        )
        .push(Space::new().height(Length::Fixed(80.0)))
        .push(
            Column::new()
                .push(
                    Column::new()
                        .push(Container::new(
                            match bitcoin_unit {
                                BitcoinDisplayUnit::BTC => {
                                    form::Form::new_amount_btc("Amount in BTC", amount, |msg| {
                                        Message::Home(HomeMessage::AmountEdited(msg))
                                    })
                                }
                                BitcoinDisplayUnit::Sats => {
                                    form::Form::new_amount_sats("Amount in sats", amount, |msg| {
                                        Message::Home(HomeMessage::AmountEdited(msg))
                                    })
                                }
                            }
                            .size(20)
                            .padding(10),
                        ))
                        .push_maybe(
                            if direction == TransferDirection::LiquidToVault {
                                onchain_send_limit
                            } else {
                                onchain_receive_limit
                            }
                            .map(|limits| {
                                Container::new(
                                    text(format!(
                                        "Enter an amount between {} and {}",
                                        Amount::from_sat(limits.0)
                                            .to_formatted_string_with_unit(bitcoin_unit),
                                        Amount::from_sat(limits.1)
                                            .to_formatted_string_with_unit(bitcoin_unit),
                                    ))
                                    .size(12),
                                )
                                .padding(7)
                            }),
                        ),
                )
                .push(button::primary(None, "Next").on_press_maybe(
                    if amount.value.is_empty() || !amount.valid {
                        None
                    } else {
                        Some(Message::Home(HomeMessage::NextStep))
                    },
                ))
                .spacing(40)
                .width(Length::Fixed(460.0)),
        )
        .width(Length::Fill)
        .align_x(Alignment::Center);

    Container::new(content)
        .padding([40, 20])
        .height(Length::Fixed(400.0))
        .width(Length::Fill)
        .style(theme::card::simple)
        .into()
}

#[allow(clippy::too_many_arguments)]
fn enter_amount_view<'a>(
    direction: TransferDirection,
    liquid_balance: &Amount,
    vault_balance: &Amount,
    fiat_converter: Option<FiatAmountConverter>,
    entered_amount: &'a form::Value<String>,
    bitcoin_unit: crate::app::settings::unit::BitcoinDisplayUnit,
    onchain_send_limit: Option<(u64, u64)>,
    onchain_receive_limit: Option<(u64, u64)>,
) -> Element<'a, Message> {
    let (from_balance, to_balance, from_name, to_name) = match direction {
        TransferDirection::LiquidToVault => (liquid_balance, vault_balance, "Liquid", "Vault"),
        TransferDirection::VaultToLiquid => (vault_balance, liquid_balance, "Vault", "Liquid"),
    };

    let cards_row = match direction {
        TransferDirection::LiquidToVault => Row::new()
            .spacing(20)
            .push(balance_summary_card(
                from_name,
                true,
                from_balance,
                fiat_converter,
                bitcoin_unit,
            ))
            .push(balance_summary_card(
                to_name,
                false,
                to_balance,
                fiat_converter,
                bitcoin_unit,
            )),
        TransferDirection::VaultToLiquid => Row::new()
            .spacing(20)
            .push(balance_summary_card(
                from_name,
                false,
                from_balance,
                fiat_converter,
                bitcoin_unit,
            ))
            .push(balance_summary_card(
                to_name,
                true,
                to_balance,
                fiat_converter,
                bitcoin_unit,
            )),
    };

    let content = Column::new()
        .push(Space::new().height(Length::Fixed(60.0)))
        .spacing(20)
        .push(
            Column::new()
                .push(
                    Row::new()
                        .push(
                            button::secondary(None, "< Previous")
                                .width(Length::Fixed(150.0))
                                .on_press(Message::Home(HomeMessage::PreviousStep)),
                        )
                        .push(
                            text("Transfer Between Wallets")
                                .bold()
                                .size(H2_SIZE)
                                .width(Length::Fill)
                                .align_x(Alignment::Center),
                        )
                        .push(Space::new().width(Length::Fixed(150.0))),
                )
                .width(Length::Fill),
        )
        .push(
            Stack::new().push(cards_row).push(
                Container::new(
                    Container::new(
                        arrow_right()
                            .width(Length::Fill)
                            .height(Length::Fill)
                            .align_x(Alignment::Center)
                            .align_y(Alignment::Center),
                    )
                    .style(|_| iced::widget::container::Style {
                        border: iced::Border {
                            color: color::ORANGE,
                            radius: 30.0.into(),
                            width: 0.5,
                        },
                        background: Some(iced::Background::Color(color::GREY_6)),
                        text_color: Some(color::ORANGE),
                        ..Default::default()
                    })
                    .height(Length::Fixed(40.0))
                    .width(Length::Fixed(40.0)),
                )
                .width(Length::Fill)
                .height(Length::Fill)
                .align_x(Alignment::Center)
                .align_y(Alignment::Center),
            ),
        )
        .push(enter_amount_card(
            direction,
            entered_amount,
            onchain_send_limit,
            onchain_receive_limit,
            bitcoin_unit,
        ))
        .padding(20)
        .width(Length::Fill)
        .align_x(Alignment::Center);

    Container::new(content)
        .width(Length::Fill)
        .center_x(Length::Fill)
        .into()
}

#[allow(clippy::too_many_arguments)]
fn confirm_transfer_view<'a>(
    direction: TransferDirection,
    amount: &'a form::Value<String>,
    receive_address: Option<&'a coincube_core::miniscript::bitcoin::Address>,
    labels: &'a std::collections::HashMap<String, String>,
    labels_editing: &'a std::collections::HashMap<String, form::Value<String>>,
    address_expanded: bool,
    is_sending: bool,
    is_tx_signed: bool,
    bitcoin_unit: BitcoinDisplayUnit,
    prepare_onchain_send_response: Option<&'a PreparePayOnchainResponse>,
    vault_to_liquid_fees: Option<Amount>,
) -> Element<'a, Message> {
    const NUM_ADDR_CHARS: usize = 16;
    let mut liquid_to_vault_fees = None;
    let amount = Amount::from_str_in(
        &amount.value,
        if matches!(bitcoin_unit, BitcoinDisplayUnit::BTC) {
            Denomination::Bitcoin
        } else {
            Denomination::Satoshi
        },
    );
    if let Some(prepare_response) = prepare_onchain_send_response {
        liquid_to_vault_fees = Some(Amount::from_sat(prepare_response.total_fees_sat));
    }

    let content = Column::new()
        .width(Length::Fill)
        .push(Space::new().height(Length::Fixed(60.0)))
        .push(
            button::secondary(None, "< Previous")
                .width(Length::Fixed(150.0))
                .on_press(Message::Home(HomeMessage::PreviousStep)),
        )
        .push(Space::new().height(Length::Fixed(20.0)))
        .push(Container::new(
            Column::new()
                .push(
                    Column::new()
                        .spacing(10)
                        .push(text("Confirm Transfer").bold().size(H2_SIZE))
                        .push(
                            Row::new()
                                .spacing(4)
                                .push(text("Sending from"))
                                .push(text(direction.display()).bold()),
                        )
                        .align_x(Alignment::Center)
                        .width(Length::Fill),
                )
                .push(Space::new().height(60))
                .push(match direction {
                    TransferDirection::LiquidToVault => Some(
                        Column::new()
                            .spacing(10)
                            .push(
                                text("Receiving Address")
                                    .bold()
                                    .width(Length::Fill)
                                    .align_x(Alignment::Center),
                            )
                            .push(receive_address.map(|addr| -> Element<'a, Message> {
                                if address_expanded {
                                    Button::new(address_card(0, addr, labels, labels_editing))
                                        .padding(0)
                                        .on_press(Message::SelectAddress(addr.clone()))
                                        .style(theme::button::transparent_border)
                                        .into()
                                } else {
                                    let addr_str = addr.to_string();
                                    let addr_len = addr_str.chars().count();

                                    Container::new(
                                        Button::new(
                                            Row::new()
                                                .spacing(10)
                                                .push(
                                                    Container::new(
                                                        p2_regular(
                                                            if addr_len > 2 * NUM_ADDR_CHARS {
                                                                format!(
                                                                    "{}...{}",
                                                                    addr_str
                                                                        .chars()
                                                                        .take(NUM_ADDR_CHARS)
                                                                        .collect::<String>(),
                                                                    addr_str
                                                                        .chars()
                                                                        .skip(
                                                                            addr_len
                                                                                - NUM_ADDR_CHARS
                                                                        )
                                                                        .collect::<String>(),
                                                                )
                                                            } else {
                                                                addr_str.clone()
                                                            },
                                                        )
                                                        .small()
                                                        .style(theme::text::secondary),
                                                    )
                                                    .padding(10)
                                                    .width(Length::Fixed(350.0)),
                                                )
                                                .push(
                                                    Container::new(
                                                        text(
                                                            labels
                                                                .get(&addr_str)
                                                                .cloned()
                                                                .unwrap_or_default(),
                                                        )
                                                        .small()
                                                        .style(theme::text::secondary),
                                                    )
                                                    .padding(10)
                                                    .width(Length::Fill),
                                                )
                                                .align_y(Alignment::Center),
                                        )
                                        .on_press(Message::SelectAddress(addr.clone()))
                                        .padding(20)
                                        .width(Length::Fill)
                                        .style(theme::button::secondary),
                                    )
                                    .style(theme::card::simple)
                                    .into()
                                }
                            }))
                            .push(receive_address.is_none().then(|| {
                                text("No receiving address available. Please generate one first.")
                                    .style(theme::text::secondary)
                            })),
                    ),
                    TransferDirection::VaultToLiquid => {
                        // TODO: This should be implemented once Liquid Wallet is done
                        Some(
                            Column::new()
                                .spacing(10)
                                .push(text("Receiving Wallet").bold())
                                .push(
                                    text("Transferring to Liquid wallet")
                                        .style(theme::text::secondary),
                                )
                                .push_maybe(receive_address.map(|addr| -> Element<'a, Message> {
                                    if address_expanded {
                                        Button::new(address_card(0, addr, labels, labels_editing))
                                            .padding(0)
                                            .on_press(Message::SelectAddress(addr.clone()))
                                            .style(theme::button::transparent_border)
                                            .into()
                                    } else {
                                        let addr_str = addr.to_string();
                                        let addr_len = addr_str.chars().count();

                                        Container::new(
                                            Button::new(
                                                Row::new()
                                                    .spacing(10)
                                                    .push(
                                                        Container::new(
                                                            p2_regular(
                                                                if addr_len > 2 * NUM_ADDR_CHARS {
                                                                    format!(
                                                                    "{}...{}",
                                                                    addr_str
                                                                        .chars()
                                                                        .take(NUM_ADDR_CHARS)
                                                                        .collect::<String>(),
                                                                    addr_str
                                                                        .chars()
                                                                        .skip(
                                                                            addr_len
                                                                                - NUM_ADDR_CHARS
                                                                        )
                                                                        .collect::<String>(),
                                                                )
                                                                } else {
                                                                    addr_str.clone()
                                                                },
                                                            )
                                                            .small()
                                                            .style(theme::text::secondary),
                                                        )
                                                        .padding(10)
                                                        .width(Length::Fixed(350.0)),
                                                    )
                                                    .push(
                                                        Container::new(
                                                            text(
                                                                labels
                                                                    .get(&addr_str)
                                                                    .cloned()
                                                                    .unwrap_or_default(),
                                                            )
                                                            .small()
                                                            .style(theme::text::secondary),
                                                        )
                                                        .padding(10)
                                                        .width(Length::Fill),
                                                    )
                                                    .align_y(Alignment::Center),
                                            )
                                            .on_press(Message::SelectAddress(addr.clone()))
                                            .padding(20)
                                            .width(Length::Fill)
                                            .style(theme::button::secondary),
                                        )
                                        .style(theme::card::simple)
                                        .into()
                                    }
                                })).
                            push_maybe(receive_address.is_none().then(|| {
                                text("No receiving address available. Please generate one first.")
                                    .style(theme::text::secondary)
                            })),
                        )
                    }
                }),
        ))
        .push(Space::new().height(Length::Fixed(20.0)))
        .push({
            Container::new(
                Row::new()
                    .padding(20)
                    .push(text("Amount:"))
                    .push(Space::new().width(Length::Fill))
                    .push_maybe(if amount.is_ok() {
                        Some(text(
                            amount
                                .clone()
                                .unwrap()
                                .to_formatted_string_with_unit(bitcoin_unit),
                        ))
                    } else {
                        None
                    }),
            )
            .width(Length::Fill)
            .style(theme::card::simple)
        })
        .push(Space::new().height(3))
        .push_maybe({
            let fees = match direction {
                TransferDirection::LiquidToVault => liquid_to_vault_fees,
                TransferDirection::VaultToLiquid => vault_to_liquid_fees,
            };

            fees.map(|fees| {
                Container::new(
                    Row::new()
                        .padding(20)
                        .push(text("Fees:"))
                        .push(Space::new().width(Length::Fill))
                        .push(text(fees.to_formatted_string_with_unit(bitcoin_unit))),
                )
                .width(Length::Fill)
                .style(theme::card::simple)
            })
        })
        .push(Space::new().height(3))
        .push_maybe({
            let fees = match direction {
                TransferDirection::LiquidToVault => liquid_to_vault_fees,
                TransferDirection::VaultToLiquid => vault_to_liquid_fees,
            };

            if let Some(fees) = fees {
                if let Ok(amount) = amount {
                    Some(
                        Container::new(
                            Row::new()
                                .padding(20)
                                .push(text("Total:"))
                                .push(Space::new().width(Length::Fill))
                                .push(text(
                                    (fees + amount).to_formatted_string_with_unit(bitcoin_unit),
                                )),
                        )
                        .width(Length::Fill)
                        .style(theme::card::simple),
                    )
                } else {
                    None
                }
            } else {
                None
            }
        })
        .push(Space::new().height(Length::Fixed(60.0)))
        .push(match direction {
            TransferDirection::VaultToLiquid => {
                if is_tx_signed {
                    button::primary(None, "Confirm & Broadcast").on_press_maybe(if !is_sending {
                        Some(Message::Home(HomeMessage::ConfirmTransfer))
                    } else {
                        None
                    })
                } else {
                    button::primary(None, "Sign Transaction").on_press_maybe(if !is_sending {
                        Some(Message::Home(HomeMessage::SignVaultToLiquidTx))
                    } else {
                        None
                    })
                }
            }
            TransferDirection::LiquidToVault => button::primary(None, "Confirm Transfer")
                .on_press_maybe(if !is_sending {
                    Some(Message::Home(HomeMessage::ConfirmTransfer))
                } else {
                    None
                }),
        });

    Container::new(content)
        .width(Length::Fill)
        .center_x(Length::Fill)
        .into()
}

pub fn transfer_successful_view<'a>(
    direction: TransferDirection,
    pending_vault_incoming: Option<PendingIncomingTransfer>,
) -> Element<'a, Message> {
    use coincube_ui::widget::{Column, Row};
    Column::new()
        .spacing(20)
        .width(Length::Fill)
        .push(Space::new().height(Length::Fixed(20.0)))
        .push(
            Row::new()
                .width(Length::Fill)
                .align_y(Alignment::Center)
                .push(Space::new().width(Length::Fill))
                .push(check_circle_icon().size(140).color(color::ORANGE))
                .push(Space::new().width(Length::Fill)),
        )
        .push(Space::new().height(Length::Fixed(16.0)))
        .push(
            Row::new()
                .width(Length::Fill)
                .align_y(Alignment::Center)
                .push(Space::new().width(Length::Fill))
                .push(
                    Column::new()
                        .width(Length::Shrink)
                        .align_x(Alignment::Center)
                        .push(h3(
                            if matches!(direction, TransferDirection::LiquidToVault)
                                && pending_vault_incoming
                                    .map(|p| p.stage != IncomingTransferStage::Completed)
                                    .unwrap_or(false)
                            {
                                "Transfer Processing"
                            } else {
                                "Transfer Successful!"
                            },
                        )),
                )
                .push(Space::new().width(Length::Fill)),
        )
        .push(
            Row::new()
                .width(Length::Fill)
                .align_y(Alignment::Center)
                .push(Space::new().width(Length::Fill))
                .push(
                    Row::new().spacing(5).push(
                        text(
                            (if matches!(direction, TransferDirection::LiquidToVault)
                                && pending_vault_incoming
                                    .map(|p| p.stage != IncomingTransferStage::Completed)
                                    .unwrap_or(false)
                            {
                                pending_vault_incoming
                                    .map(|pending| {
                                        format!(
                                            "Funds are on the way to Vault. Current step: {}",
                                            incoming_transfer_status_text(pending.stage)
                                        )
                                    })
                                    .unwrap_or_else(|| "Funds are on the way to Vault".to_string())
                            } else {
                                format!(
                                    "Your funds have been moved to your {} Wallet",
                                    if matches!(direction, TransferDirection::LiquidToVault) {
                                        "Vault"
                                    } else {
                                        "Liquid"
                                    }
                                )
                            })
                            .to_string(),
                        )
                        .size(20),
                    ),
                )
                .push(Space::new().width(Length::Fill)),
        )
        .push(Space::new().height(Length::Fixed(20.0)))
        .push(
            Row::new()
                .width(Length::Fill)
                .align_y(Alignment::Center)
                .push(Space::new().width(Length::Fill))
                .push(
                    button::primary(None, "Back")
                        .width(Length::Fixed(150.0))
                        .on_press(Message::Home(HomeMessage::BackToHome)),
                )
                .push(Space::new().width(Length::Fill)),
        )
        .into()
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TransferDirection {
    LiquidToVault,
    VaultToLiquid,
}

impl TransferDirection {
    pub fn from_wallet(&self) -> &'static str {
        match self {
            Self::LiquidToVault => "Liquid",
            Self::VaultToLiquid => "Vault",
        }
    }

    pub fn to_wallet(&self) -> &'static str {
        match self {
            Self::LiquidToVault => "Vault",
            Self::VaultToLiquid => "Liquid",
        }
    }

    pub fn display(&self) -> String {
        format!("{} → {}", self.from_wallet(), self.to_wallet())
    }
}

pub struct GlobalViewConfig<'a> {
    pub liquid_balance: Amount,
    pub usdt_balance: u64,
    pub usdt_balance_error: bool,
    pub pending_liquid_send_sats: u64,
    pub pending_usdt_send_sats: u64,
    pub pending_liquid_receive_sats: u64,
    pub pending_usdt_receive_sats: u64,
    pub vault_pending_send_sats: u64,
    pub vault_pending_receive_sats: u64,
    pub vault_balance: Amount,
    pub fiat_converter: Option<FiatAmountConverter>,
    pub balance_masked: bool,
    pub has_vault: bool,
    pub current_view: HomeView,
    pub transfer_direction: Option<TransferDirection>,
    pub entered_amount: &'a form::Value<String>,
    pub receive_address: Option<&'a coincube_core::miniscript::bitcoin::Address>,
    pub receive_index: Option<&'a coincube_core::miniscript::bitcoin::bip32::ChildNumber>,
    pub labels: &'a std::collections::HashMap<String, String>,
    pub labels_editing: &'a std::collections::HashMap<String, form::Value<String>>,
    pub address_expanded: bool,
    pub bitcoin_unit: crate::app::settings::unit::BitcoinDisplayUnit,
    pub onchain_send_limit: Option<(u64, u64)>,
    pub onchain_receive_limit: Option<(u64, u64)>,
    pub is_sending: bool,
    pub is_tx_signed: bool,
    pub prepare_onchain_send_response: Option<&'a PreparePayOnchainResponse>,
    pub spend_tx_fees: Option<Amount>,
    pub pending_vault_incoming: Option<PendingIncomingTransfer>,
    pub pending_animation_phase: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HomeView {
    pub step: usize,
}

impl HomeView {
    pub fn next(&mut self) {
        self.step += 1;
    }

    pub fn previous(&mut self) {
        if self.step > 0 {
            self.step -= 1;
        }
    }

    pub fn reset(&mut self) {
        self.step = 0;
    }
}

pub fn global_home_view<'a>(config: GlobalViewConfig<'a>) -> Element<'a, Message> {
    let GlobalViewConfig {
        liquid_balance,
        usdt_balance,
        usdt_balance_error,
        pending_liquid_send_sats,
        pending_usdt_send_sats,
        pending_liquid_receive_sats,
        pending_usdt_receive_sats,
        vault_pending_send_sats,
        vault_pending_receive_sats,
        vault_balance,
        fiat_converter,
        balance_masked,
        has_vault,
        current_view,
        transfer_direction,
        entered_amount,
        receive_address,
        receive_index: _receive_index,
        labels,
        labels_editing,
        address_expanded,
        bitcoin_unit,
        onchain_send_limit,
        onchain_receive_limit,
        is_sending,
        is_tx_signed,
        prepare_onchain_send_response,
        spend_tx_fees,
        pending_vault_incoming,
        pending_animation_phase,
    } = config;

    match current_view.step {
        1 => {
            return select_transfer_direction_view(transfer_direction);
        }
        2 => {
            if let Some(direction) = transfer_direction {
                return enter_amount_view(
                    direction,
                    &liquid_balance,
                    &vault_balance,
                    fiat_converter,
                    entered_amount,
                    bitcoin_unit,
                    onchain_send_limit,
                    onchain_receive_limit,
                );
            }
        }
        3 => {
            if let Some(direction) = transfer_direction {
                return confirm_transfer_view(
                    direction,
                    entered_amount,
                    receive_address,
                    labels,
                    labels_editing,
                    address_expanded,
                    is_sending,
                    is_tx_signed,
                    bitcoin_unit,
                    prepare_onchain_send_response,
                    spend_tx_fees,
                );
            }
        }
        4 => {
            if let Some(direction) = transfer_direction {
                return transfer_successful_view(direction, pending_vault_incoming);
            }
        }
        0 => {}
        _ => {}
    }

    let liquid_card = mouse_area(wallet_card(
        WalletType::Liquid,
        &liquid_balance,
        fiat_converter,
        balance_masked,
        false,
        bitcoin_unit,
        pending_vault_incoming,
        pending_animation_phase,
        pending_liquid_send_sats,
        pending_liquid_receive_sats,
    ))
    .on_press(Message::Menu(Menu::Liquid(LiquidSubMenu::Overview)));

    let usdt_card = mouse_area(wallet_card(
        WalletType::Usdt {
            balance: usdt_balance,
            error: usdt_balance_error,
        },
        &Amount::ZERO,
        fiat_converter,
        balance_masked,
        false,
        bitcoin_unit,
        None,
        0.0,
        pending_usdt_send_sats,
        pending_usdt_receive_sats,
    ))
    .on_press(Message::Menu(Menu::Usdt(UsdtSubMenu::Overview)));

    let vault_card_element = mouse_area(wallet_card(
        WalletType::Vault,
        &vault_balance,
        fiat_converter,
        balance_masked,
        has_vault,
        bitcoin_unit,
        pending_vault_incoming,
        pending_animation_phase,
        vault_pending_send_sats,
        vault_pending_receive_sats,
    ))
    .on_press(Message::Menu(Menu::Vault(VaultSubMenu::Overview)));

    Column::new()
        .spacing(20)
        .push(
            Row::new()
                .spacing(0)
                .width(Length::Fill)
                .push(h3("Wallets").bold())
                .push(
                    Button::new(if balance_masked {
                        eye_slash_icon()
                    } else {
                        eye_outline_icon()
                    })
                    .style(theme::button::container)
                    .on_press(Message::Home(HomeMessage::ToggleBalanceMask)),
                )
                .align_y(Alignment::Center),
        )
        .push(
            Column::new()
                .spacing(40)
                .push(liquid_card)
                .push(usdt_card)
                .push(vault_card_element),
        )
        .into()
}

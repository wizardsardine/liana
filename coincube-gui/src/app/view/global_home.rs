use crate::app::breez_liquid::assets::format_usdt_display;
use crate::app::{
    menu::Menu,
    view::{vault::receive::address_card, FiatAmountConverter},
};
use crate::app::{
    menu::{LiquidSubMenu, VaultSubMenu},
    view::message::{HomeMessage, Message},
};
use breez_sdk_liquid::{bitcoin::Denomination, model::PreparePayOnchainResponse};
use coincube_core::miniscript::bitcoin::Amount;
use coincube_ui::{
    color,
    component::{amount::*, button, form, text::*},
    icon::{
        arrow_down_up_icon, arrow_right, check_circle_icon, droplet_fill_icon, eye_outline_icon,
        eye_slash_icon, lightning_icon, usd_icon, vault_icon, warning_icon,
    },
    theme,
    widget::*,
};
use iced::{
    widget::{mouse_area, Button, Column, Space, Stack},
    Alignment, Length,
};

#[derive(Clone, Copy, Debug)]
#[allow(dead_code)]
enum WalletType {
    Liquid,
    Usdt { balance: u64, error: bool },
    Vault,
}

/// Stages a transfer passes through between `Initiated` and `Completed`.
///
/// All six directed pairs land on `PendingDeposit` from the UI's perspective —
/// the success screen (`transfer_successful_view`) renders the "Pending Deposit"
/// treatment at that point and the flow exits. The home-page pending indicators
/// clear asynchronously once the destination's confirmation requirements are met.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransferStage {
    Initiated,
    SwappingLbtcToBtc,
    /// Used by any→Liquid leg: after the on-chain tx confirms at the Breez
    /// peg-in swap address, Breez credits L-BTC and we transition to `Completed`.
    SwappingBtcToLbtc,
    /// The source wallet is broadcasting its on-chain tx. Short-lived — flips to
    /// `PendingDeposit` as soon as we have a txid.
    BroadcastingOnChain,
    /// The on-chain tx is broadcast and awaiting confirmations at the destination.
    /// This is the terminal state for the success-screen UX.
    PendingDeposit,
    /// Breez is forwarding the swapped BTC to the Vault address (L-BTC→Vault leg).
    SendingToVault,
    Completed,
}

#[derive(Clone, Copy, Debug)]
pub struct PendingTransfer {
    pub amount: Amount,
    pub stage: TransferStage,
}

/// A quiet, non-animated "Pending deposit" indicator rendered on a wallet card
/// while a transfer is awaiting confirmations at that destination. Confirmation
/// delays are unbounded from the UI's perspective — no progress bar, no stage
/// animation.
fn pending_deposit_card<'a>(
    pending_transfer: PendingTransfer,
    bitcoin_unit: BitcoinDisplayUnit,
) -> Element<'a, Message> {
    Container::new(
        Row::new()
            .spacing(8)
            .align_y(Alignment::Center)
            .push(warning_icon().size(12).style(theme::text::secondary))
            .push(text("Pending deposit").size(12).color(color::GREY_2))
            .push(Space::new().width(Length::Fill))
            .push(
                text(
                    pending_transfer
                        .amount
                        .to_formatted_string_with_unit(bitcoin_unit),
                )
                .size(12)
                .color(color::ORANGE),
            ),
    )
    .padding([10, 14])
    .style(|t| iced::widget::container::Style {
        border: iced::Border {
            color: t.colors.cards.simple.border.unwrap_or(color::GREY_4),
            width: 0.5,
            radius: 16.0.into(),
        },
        background: Some(iced::Background::Color(t.colors.cards.simple.background)),
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
    pending_vault_incoming: Option<PendingTransfer>,
    pending_send_sats: u64,
    pending_receive_sats: u64,
) -> Element<'a, Message> {
    let fiat_balance = fiat_converter.as_ref().map(|c| c.convert(*balance));

    let (icon, title, title_color, send_action, receive_action) = match wallet_type {
        WalletType::Liquid => (
            droplet_fill_icon().style(theme::text::secondary),
            "Liquid",
            None::<iced::Color>,
            Message::Menu(Menu::Liquid(LiquidSubMenu::Send)),
            Message::Menu(Menu::Liquid(LiquidSubMenu::Receive)),
        ),
        WalletType::Usdt { .. } => (
            usd_icon().style(theme::text::secondary),
            "USDt",
            None,
            Message::Menu(Menu::Liquid(LiquidSubMenu::Send)),
            Message::Menu(Menu::Liquid(LiquidSubMenu::Receive)),
        ),
        WalletType::Vault => (
            vault_icon().style(theme::text::secondary),
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
                            .on_press(Message::Menu(Menu::Liquid(LiquidSubMenu::Send))),
                    )
                    .push(Space::new().width(Length::Fixed(8.0)))
                    .push(
                        button::orange_outline(None, "Receive")
                            .width(Length::Fixed(120.0))
                            .on_press(Message::Menu(Menu::Liquid(LiquidSubMenu::Receive))),
                    ),
            );
        return Container::new(content)
            .padding(20)
            .style(|t| iced::widget::container::Style {
                border: iced::Border {
                    color: color::ORANGE,
                    width: 0.2,
                    radius: 25.0.into(),
                },
                background: Some(iced::Background::Color(t.colors.cards.simple.background)),
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
                    .push({
                        let t = text(title).size(14);
                        if let Some(c) = title_color {
                            t.color(c)
                        } else {
                            t.style(theme::text::secondary)
                        }
                    }),
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
                                fiat_balance.map(|fiat| {
                                    fiat.to_text().size(P1_SIZE).style(theme::text::secondary)
                                })
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
                    .push(
                        button::primary(None, "Send")
                            .width(Length::Fixed(120.0))
                            .on_press(send_action),
                    )
                    .push(Space::new().width(Length::Fixed(8.0)))
                    .push(
                        button::orange_outline(None, "Receive")
                            .width(Length::Fixed(120.0))
                            .on_press(receive_action),
                    ),
            ),
    };

    let content = if matches!(wallet_type, WalletType::Vault) {
        if let Some(pending_transfer) = pending_vault_incoming {
            if pending_transfer.stage != TransferStage::Completed {
                content.push(pending_deposit_card(pending_transfer, bitcoin_unit))
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
                background: Some(iced::Background::Color(t.colors.cards.simple.background)),
                ..Default::default()
            },
            WalletType::Usdt { .. } => iced::widget::container::Style {
                border: iced::Border {
                    color: color::ORANGE,
                    width: 0.2,
                    radius: 25.0.into(),
                },
                background: Some(iced::Background::Color(t.colors.cards.simple.background)),
                ..Default::default()
            },
            WalletType::Vault => theme::card::simple(t),
        })
        .into()
}

fn wallet_kind_icon<'a, M>(kind: WalletKind, size: f32) -> Element<'a, M>
where
    M: 'a,
{
    match kind {
        WalletKind::Liquid => droplet_fill_icon()
            .size(size)
            .style(theme::text::secondary)
            .into(),
        WalletKind::Spark => lightning_icon()
            .size(size)
            .style(theme::text::secondary)
            .into(),
        WalletKind::Vault => vault_icon().size(size).style(theme::text::secondary).into(),
    }
}

/// One of the two clickable wallet-summary cards on the amount-entry screen.
/// Pressing it opens the wallet-picker popup to edit the named side.
fn balance_summary_card<'a>(
    kind: WalletKind,
    balance: &Amount,
    fiat_converter: Option<FiatAmountConverter>,
    bitcoin_unit: crate::app::settings::unit::BitcoinDisplayUnit,
    on_press: Message,
) -> Element<'a, Message> {
    let fiat_balance = fiat_converter.as_ref().map(|c| c.convert(*balance));
    let name = kind.label();

    let content = Column::new()
        .spacing(12)
        .push(
            Row::new()
                .spacing(8)
                .align_y(Alignment::Center)
                .push(wallet_kind_icon::<Message>(kind, 16.0))
                .push(text(name).color(color::GREY_2).size(14)),
        )
        .push(
            Row::new().align_y(Alignment::Center).push(
                Column::new()
                    .spacing(4)
                    .push(amount_with_size_and_unit(balance, H2_SIZE, bitcoin_unit))
                    .push_maybe(
                        fiat_balance
                            .map(|fiat| fiat.to_text().size(P1_SIZE).style(theme::text::secondary)),
                    ),
            ),
        );

    // Orange-outlined style for Liquid (matches the historic home-card treatment);
    // plain card style for Spark/Vault.
    let is_liquid = matches!(kind, WalletKind::Liquid);
    let card = Container::new(content)
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
                    background: Some(iced::Background::Color(t.colors.cards.simple.background)),
                    ..Default::default()
                }
            } else {
                theme::card::simple(t)
            }
        });

    Button::new(card)
        .padding(0)
        .width(Length::Fill)
        .style(theme::button::transparent_border)
        .on_press(on_press)
        .into()
}

/// Popup content for the wallet-picker modal. Lists each available wallet as a
/// `picker_row`; the row matching the opposite side is disabled (non-pressable)
/// to prevent the illegal same-wallet selection.
#[allow(clippy::too_many_arguments)]
fn wallet_selector_popup<'a>(
    from: Option<WalletKind>,
    to: Option<WalletKind>,
    editing: PickerSide,
    has_vault: bool,
    has_spark: bool,
    liquid_balance: Amount,
    spark_balance: Amount,
    vault_balance: Amount,
    bitcoin_unit: BitcoinDisplayUnit,
) -> Element<'a, Message> {
    let title_label = match editing {
        PickerSide::From => "FROM",
        PickerSide::To => "TO",
    };

    let current = match editing {
        PickerSide::From => from,
        PickerSide::To => to,
    };

    // When editing the TO side, hide whatever wallet is currently set as FROM —
    // listing it disabled is noise when the user already knows they're
    // transferring *from* it. When editing the FROM side, show all three: the
    // user may want to swap which wallet is the source, and seeing the current
    // TO wallet in the FROM list is useful for that (picking it implicitly
    // swaps sides in the state layer).
    let hidden = match editing {
        PickerSide::From => None,
        PickerSide::To => from,
    };

    let row_for = |kind: WalletKind, balance: Amount| -> Element<'a, Message> {
        let is_selected = current == Some(kind);
        let balance_str = balance.to_formatted_string_with_unit(bitcoin_unit);
        let icon_elem = wallet_kind_icon::<Message>(kind, 36.0);
        crate::app::view::shared::picker::picker_row(
            icon_elem,
            kind.label(),
            &balance_str,
            kind.badge(),
            is_selected,
            Message::Home(HomeMessage::SelectWalletInPicker(kind)),
        )
    };

    let mut col = Column::new()
        .spacing(16)
        .padding(24)
        .max_width(420)
        .push(text(title_label).size(H4_SIZE).bold());

    if hidden != Some(WalletKind::Liquid) {
        col = col.push(row_for(WalletKind::Liquid, liquid_balance));
    }
    if has_spark && hidden != Some(WalletKind::Spark) {
        col = col.push(row_for(WalletKind::Spark, spark_balance));
    }
    if has_vault && hidden != Some(WalletKind::Vault) {
        col = col.push(row_for(WalletKind::Vault, vault_balance));
    }

    col.into()
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
                        .push({
                            // Helper text reflects the per-direction effective
                            // minimum (§Design section 4) composed with Breez's
                            // SDK limits on swap-involving legs.
                            let effective_min = crate::app::state::effective_transfer_min_sat(
                                direction,
                                onchain_send_limit,
                                onchain_receive_limit,
                            );
                            let effective_max = crate::app::state::effective_transfer_max_sat(
                                direction,
                                onchain_send_limit,
                                onchain_receive_limit,
                            );
                            Container::new(
                                text(match (effective_min, effective_max) {
                                    (Some(min), Some(max)) => format!(
                                        "Enter an amount between {} and {}",
                                        Amount::from_sat(min)
                                            .to_formatted_string_with_unit(bitcoin_unit),
                                        Amount::from_sat(max)
                                            .to_formatted_string_with_unit(bitcoin_unit),
                                    ),
                                    (Some(min), None) => format!(
                                        "Minimum transfer: {}",
                                        Amount::from_sat(min)
                                            .to_formatted_string_with_unit(bitcoin_unit),
                                    ),
                                    _ => "Loading limits…".to_string(),
                                })
                                .size(12),
                            )
                            .padding(7)
                        }),
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
    spark_balance: &Amount,
    vault_balance: &Amount,
    fiat_converter: Option<FiatAmountConverter>,
    entered_amount: &'a form::Value<String>,
    bitcoin_unit: crate::app::settings::unit::BitcoinDisplayUnit,
    onchain_send_limit: Option<(u64, u64)>,
    onchain_receive_limit: Option<(u64, u64)>,
) -> Element<'a, Message> {
    let balance_for = |kind: WalletKind| -> &Amount {
        match kind {
            WalletKind::Liquid => liquid_balance,
            WalletKind::Spark => spark_balance,
            WalletKind::Vault => vault_balance,
        }
    };

    let from_kind = direction.from_kind();
    let to_kind = direction.to_kind();

    let cards_row = Row::new()
        .spacing(20)
        .push(balance_summary_card(
            from_kind,
            balance_for(from_kind),
            fiat_converter,
            bitcoin_unit,
            Message::Home(HomeMessage::OpenWalletPicker(PickerSide::From)),
        ))
        .push(balance_summary_card(
            to_kind,
            balance_for(to_kind),
            fiat_converter,
            bitcoin_unit,
            Message::Home(HomeMessage::OpenWalletPicker(PickerSide::To)),
        ));

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
                    .style(|t| iced::widget::container::Style {
                        border: iced::Border {
                            color: color::ORANGE,
                            radius: 30.0.into(),
                            width: 0.5,
                        },
                        background: Some(iced::Background::Color(t.colors.cards.simple.background)),
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
    transfer_feerate: &'a form::Value<String>,
    transfer_feerate_loading: Option<crate::app::view::shared::feerate_picker::FeeratePreset>,
    spark_send_fee_sat: Option<u64>,
) -> Element<'a, Message> {
    let spark_fee_amount = spark_send_fee_sat.map(Amount::from_sat);
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
                    TransferDirection::LiquidToVault | TransferDirection::SparkToVault => Some(
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
                    TransferDirection::VaultToLiquid
                    | TransferDirection::SparkToLiquid
                    | TransferDirection::LiquidToSpark
                    | TransferDirection::VaultToSpark => {
                        let destination_label = direction.to_wallet();
                        Some(
                            Column::new()
                                .spacing(10)
                                .push(text("Receiving Wallet").bold())
                                .push(
                                    text(format!(
                                        "Transferring to {destination_label} wallet"
                                    ))
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
        .push_maybe((direction.from_kind() == WalletKind::Vault).then(|| {
            // §Design section 9: Vault-sourced transfers build a standard on-chain
            // Bitcoin tx, so the user controls the sats/vbyte rate. Presets fetch
            // a mempool estimate and write it into the text input.
            Container::new(
                Column::new()
                    .padding(20)
                    .spacing(10)
                    .push(text("Feerate (sats/vbyte):"))
                    .push(
                        crate::app::view::shared::feerate_picker::feerate_presets_row::<Message>(
                            transfer_feerate_loading,
                            |preset| Message::Home(HomeMessage::FetchTransferFeeratePreset(preset)),
                        ),
                    )
                    .push(crate::app::view::shared::feerate_picker::feerate_input::<
                        Message,
                        _,
                    >(transfer_feerate, |s| {
                        Message::Home(HomeMessage::SetTransferFeerate(s))
                    })),
            )
            .width(Length::Fill)
            .style(theme::card::simple)
        }))
        .push(Space::new().height(3))
        .push_maybe({
            let fees = match direction {
                TransferDirection::LiquidToVault | TransferDirection::LiquidToSpark => {
                    liquid_to_vault_fees
                }
                TransferDirection::VaultToLiquid | TransferDirection::VaultToSpark => {
                    vault_to_liquid_fees
                }
                // Spark-sourced: `spark.prepare_send` quoted a `fee_sat` at step
                // 1→2 and the state layer stashed it on `spark_send_fee_sat`.
                TransferDirection::SparkToLiquid | TransferDirection::SparkToVault => {
                    spark_fee_amount
                }
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
                TransferDirection::LiquidToVault | TransferDirection::LiquidToSpark => {
                    liquid_to_vault_fees
                }
                TransferDirection::VaultToLiquid | TransferDirection::VaultToSpark => {
                    vault_to_liquid_fees
                }
                // Spark-sourced: `spark.prepare_send` quoted a `fee_sat` at step
                // 1→2 and the state layer stashed it on `spark_send_fee_sat`.
                TransferDirection::SparkToLiquid | TransferDirection::SparkToVault => {
                    spark_fee_amount
                }
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
            // Vault-sourced: sign the on-chain tx first, then broadcast. Shared
            // signer path handles both Liquid and Spark destinations.
            TransferDirection::VaultToLiquid | TransferDirection::VaultToSpark => {
                if is_tx_signed {
                    button::primary(None, "Confirm & Broadcast").on_press_maybe(if !is_sending {
                        Some(Message::Home(HomeMessage::ConfirmTransfer))
                    } else {
                        None
                    })
                } else {
                    button::primary(None, "Sign Transaction").on_press_maybe(
                        if !is_sending && transfer_feerate.valid {
                            Some(Message::Home(HomeMessage::SignVaultToLiquidTx))
                        } else {
                            None
                        },
                    )
                }
            }
            TransferDirection::LiquidToVault | TransferDirection::LiquidToSpark => {
                button::primary(None, "Confirm Transfer").on_press_maybe(if !is_sending {
                    Some(Message::Home(HomeMessage::ConfirmTransfer))
                } else {
                    None
                })
            }
            // Spark-sourced: the prepare handle was fetched at step 1→2. Confirm
            // calls `spark.send_payment(handle)` synchronously from the UI's
            // perspective; stage flips to PendingDeposit on success.
            TransferDirection::SparkToLiquid | TransferDirection::SparkToVault => {
                button::primary(None, "Confirm Transfer").on_press_maybe(if !is_sending {
                    Some(Message::Home(HomeMessage::ConfirmSparkSend))
                } else {
                    None
                })
            }
        });

    Container::new(content)
        .width(Length::Fill)
        .center_x(Length::Fill)
        .into()
}

/// "Pending Deposit" success screen.
///
/// Every transfer direction settles via an on-chain hop, so no destination is
/// instant — the broadcast landing is the most the user can know synchronously.
/// The home-page pending indicator on the destination wallet picks up from here
/// and clears asynchronously once the destination's confirmation requirements
/// are met.
pub fn transfer_successful_view<'a>(
    direction: TransferDirection,
    _pending_vault_incoming: Option<PendingTransfer>,
) -> Element<'a, Message> {
    use coincube_ui::widget::{Column, Row};
    let destination = direction.to_wallet();

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
                        .push(h3("Transfer broadcast")),
                )
                .push(Space::new().width(Length::Fill)),
        )
        .push(
            Row::new()
                .width(Length::Fill)
                .align_y(Alignment::Center)
                .push(Space::new().width(Length::Fill))
                .push(
                    Container::new(
                        text(format!(
                            "Your transfer has been broadcast on-chain. \
                             It will appear in your {destination} wallet \
                             once it confirms. You can close this screen — \
                             we'll update the wallet when the deposit is ready."
                        ))
                        .size(P1_SIZE)
                        .style(theme::text::secondary)
                        .align_x(Alignment::Center),
                    )
                    .max_width(520),
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
                    button::primary(None, "Done")
                        .width(Length::Fixed(150.0))
                        .on_press(Message::Home(HomeMessage::BackToHome)),
                )
                .push(Space::new().width(Length::Fill)),
        )
        .into()
}

/// One of the three on-dashboard wallets. Used to address the source and destination
/// of a transfer independently of which specific `TransferDirection` is active.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WalletKind {
    Liquid,
    Spark,
    Vault,
}

impl WalletKind {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Liquid => "Liquid",
            Self::Spark => "Spark",
            Self::Vault => "Vault",
        }
    }

    /// The uppercase network badge rendered in the wallet picker rows (matching the
    /// "LIQUID"/"BITCOIN" badges used by the Liquid Send picker).
    pub fn badge(&self) -> &'static str {
        match self {
            Self::Liquid => "LIQUID",
            Self::Spark => "SPARK",
            Self::Vault => "VAULT",
        }
    }
}

/// Which side of the From/To transfer pair is being edited in the wallet picker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickerSide {
    From,
    To,
}

/// An ordered (from, to) pair of wallets the user can transfer between.
///
/// Exhaustive over the six legal directed pairs of the three wallets. Using an explicit
/// enum over a `struct { from: WalletKind, to: WalletKind }` gives compiler-driven
/// exhaustiveness in match arms and rules out the illegal `from == to` state at compile
/// time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransferDirection {
    LiquidToVault,
    VaultToLiquid,
    LiquidToSpark,
    SparkToLiquid,
    VaultToSpark,
    SparkToVault,
}

impl TransferDirection {
    pub fn pair(&self) -> (WalletKind, WalletKind) {
        match self {
            Self::LiquidToVault => (WalletKind::Liquid, WalletKind::Vault),
            Self::VaultToLiquid => (WalletKind::Vault, WalletKind::Liquid),
            Self::LiquidToSpark => (WalletKind::Liquid, WalletKind::Spark),
            Self::SparkToLiquid => (WalletKind::Spark, WalletKind::Liquid),
            Self::VaultToSpark => (WalletKind::Vault, WalletKind::Spark),
            Self::SparkToVault => (WalletKind::Spark, WalletKind::Vault),
        }
    }

    pub fn from_kind(&self) -> WalletKind {
        self.pair().0
    }

    pub fn to_kind(&self) -> WalletKind {
        self.pair().1
    }

    /// Returns `None` if `from == to` (illegal — same wallet on both sides).
    /// Availability of each wallet on the current cube is the caller's concern.
    pub fn try_from_pair(from: WalletKind, to: WalletKind) -> Option<Self> {
        Some(match (from, to) {
            (WalletKind::Liquid, WalletKind::Vault) => Self::LiquidToVault,
            (WalletKind::Vault, WalletKind::Liquid) => Self::VaultToLiquid,
            (WalletKind::Liquid, WalletKind::Spark) => Self::LiquidToSpark,
            (WalletKind::Spark, WalletKind::Liquid) => Self::SparkToLiquid,
            (WalletKind::Vault, WalletKind::Spark) => Self::VaultToSpark,
            (WalletKind::Spark, WalletKind::Vault) => Self::SparkToVault,
            _ => return None,
        })
    }

    pub fn from_wallet(&self) -> &'static str {
        self.from_kind().label()
    }

    pub fn to_wallet(&self) -> &'static str {
        self.to_kind().label()
    }

    pub fn display(&self) -> String {
        format!("{} → {}", self.from_wallet(), self.to_wallet())
    }
}

pub struct GlobalViewConfig<'a> {
    pub liquid_balance: Amount,
    /// Spark wallet BTC balance in sats. Defaults to `ZERO` while
    /// the first `get_info` round-trip is in flight or if the
    /// bridge subprocess is currently down.
    pub spark_balance: Amount,
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
    /// Whether this cube has a working Spark backend. Mirrors
    /// `spark_backend.is_some()` in the state layer. Drives the Spark card
    /// visibility and is part of the `has_vault || has_spark` gate on the
    /// Transfer button (a cube with only Liquid has nothing to transfer with).
    pub has_spark: bool,
    pub current_view: HomeView,
    pub transfer_direction: Option<TransferDirection>,
    pub transfer_from: Option<WalletKind>,
    pub transfer_to: Option<WalletKind>,
    /// When `Some`, the wallet-picker popup is open and editing the named side.
    pub wallet_picker: Option<PickerSide>,
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
    /// Vault-sourced transfers expose a sats/vbyte input on the confirm screen.
    /// Ignored on all other directions (SDK picks the fee).
    pub transfer_feerate: &'a form::Value<String>,
    /// Which Fast/Normal/Slow preset is currently fetching a mempool-driven
    /// estimate. The button for the loading preset renders non-pressable.
    pub transfer_feerate_loading: Option<crate::app::view::shared::feerate_picker::FeeratePreset>,
    /// Spark-quoted on-chain fee for the prepared send (Spark-sourced
    /// directions only). Rendered in the Fees row on the confirm screen.
    pub spark_send_fee_sat: Option<u64>,
    /// Mirror of `pending_vault_incoming` for the Spark card. Populated by the
    /// state layer when a transfer into Spark has broadcast on-chain and is
    /// awaiting maturity + claim on the Spark side.
    pub pending_spark_incoming: Option<PendingTransfer>,
    pub pending_vault_incoming: Option<PendingTransfer>,
    /// BTC price in USD for accurate USDt→sats conversion regardless of user's fiat currency.
    pub btc_usd_price: Option<f64>,
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
        spark_balance,
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
        has_spark,
        current_view,
        transfer_direction,
        transfer_from,
        transfer_to,
        wallet_picker,
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
        transfer_feerate,
        transfer_feerate_loading,
        spark_send_fee_sat,
        pending_vault_incoming,
        pending_spark_incoming,
        btc_usd_price,
    } = config;

    // Post-Phase-3 step machine:
    //   0 — overview (wallet cards + page-level Transfer button)
    //   1 — amount entry (may overlay the wallet-picker popup)
    //   2 — confirm
    //   3 — success
    match current_view.step {
        1 => {
            if let Some(direction) = transfer_direction {
                let amount_view = enter_amount_view(
                    direction,
                    &liquid_balance,
                    &spark_balance,
                    &vault_balance,
                    fiat_converter,
                    entered_amount,
                    bitcoin_unit,
                    onchain_send_limit,
                    onchain_receive_limit,
                );
                if let Some(editing) = wallet_picker {
                    let popup = wallet_selector_popup(
                        transfer_from,
                        transfer_to,
                        editing,
                        has_vault,
                        has_spark,
                        liquid_balance,
                        spark_balance,
                        vault_balance,
                        bitcoin_unit,
                    );
                    return coincube_ui::widget::modal::Modal::new(amount_view, popup)
                        .on_blur(Some(Message::Home(HomeMessage::CloseWalletPicker)))
                        .into();
                }
                return amount_view;
            }
        }
        2 => {
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
                    transfer_feerate,
                    transfer_feerate_loading,
                    spark_send_fee_sat,
                );
            }
        }
        3 => {
            if let Some(direction) = transfer_direction {
                return transfer_successful_view(direction, pending_vault_incoming);
            }
        }
        0 => {}
        _ => {}
    }

    // --- Combined Liquid card (L-BTC + USDt) ---
    // USDt is pegged to USD, so always use BTC/USD price for conversion.
    // When the user's fiat currency is USD, btc_usd_price == converter.price_per_btc().
    // When it differs (e.g. EUR), we must use the dedicated USD price to avoid mispricing.
    let usdt_fiat_value = usdt_balance as f64 / 1e8; // USDt base units → dollars
    let usdt_as_sats = if let Some(btc_price_usd) = btc_usd_price {
        if btc_price_usd > 0.0 {
            (usdt_fiat_value / btc_price_usd * 1e8) as u64
        } else {
            0
        }
    } else {
        0
    };
    let total_sats = liquid_balance.to_sat() + usdt_as_sats;
    let total_amount = Amount::from_sat(total_sats);
    let total_fiat = fiat_converter.as_ref().map(|c| c.convert(total_amount));
    let lbtc_fiat = fiat_converter.as_ref().map(|c| c.convert(liquid_balance));

    let orange_outline_btn = |label: &'static str, msg: Message| -> Element<'a, Message> {
        button::orange_outline(None, label)
            .width(Length::Fixed(90.0))
            .on_press(msg)
            .into()
    };

    // L-BTC asset row
    let lbtc_row = Column::new()
        .spacing(4)
        .push(
            Row::new()
                .spacing(8)
                .align_y(Alignment::Center)
                .push(coincube_ui::image::asset_network_logo::<Message>(
                    "lbtc", "liquid", 28.0,
                ))
                .push(
                    text("L-BTC")
                        .size(P1_SIZE)
                        .style(theme::text::secondary)
                        .width(Length::Fixed(60.0)),
                )
                .push(if balance_masked {
                    Row::new().push(text("********").size(P1_SIZE))
                } else {
                    amount_with_size_and_unit(&liquid_balance, P1_SIZE, bitcoin_unit)
                })
                .push_maybe(
                    (!balance_masked)
                        .then(|| {
                            lbtc_fiat
                                .map(|f| f.to_text().size(P2_SIZE).style(theme::text::secondary))
                        })
                        .flatten(),
                )
                .push(Space::new().width(Length::Fill))
                .push(
                    button::primary(None, "Send")
                        .width(Length::Fixed(90.0))
                        .on_press(Message::Home(HomeMessage::SendAsset(
                            crate::app::state::liquid::send::SendAsset::Lbtc,
                        ))),
                )
                .push(orange_outline_btn(
                    "Receive",
                    Message::Home(HomeMessage::ReceiveAsset(
                        crate::app::state::liquid::send::SendAsset::Lbtc,
                    )),
                )),
        )
        .push_maybe((!balance_masked && pending_liquid_send_sats > 0).then(|| {
            Row::new()
                .spacing(6)
                .align_y(Alignment::Center)
                .push(warning_icon().size(12).style(theme::text::secondary))
                .push(text("-").size(P2_SIZE).style(theme::text::secondary))
                .push(amount_with_size_and_unit(
                    &Amount::from_sat(pending_liquid_send_sats),
                    P2_SIZE,
                    bitcoin_unit,
                ))
                .push(text("pending").size(P2_SIZE).style(theme::text::secondary))
        }))
        .push_maybe(
            (!balance_masked && pending_liquid_receive_sats > 0).then(|| {
                Row::new()
                    .spacing(6)
                    .align_y(Alignment::Center)
                    .push(warning_icon().size(12).style(theme::text::secondary))
                    .push(text("+").size(P2_SIZE).style(theme::text::secondary))
                    .push(amount_with_size_and_unit(
                        &Amount::from_sat(pending_liquid_receive_sats),
                        P2_SIZE,
                        bitcoin_unit,
                    ))
                    .push(text("pending").size(P2_SIZE).style(theme::text::secondary))
            }),
        );

    // USDt asset row
    let usdt_row = Column::new()
        .spacing(4)
        .push(
            Row::new()
                .spacing(8)
                .align_y(Alignment::Center)
                .push(coincube_ui::image::asset_network_logo::<Message>(
                    "usdt", "liquid", 28.0,
                ))
                .push(
                    text("USDt")
                        .size(P1_SIZE)
                        .style(theme::text::secondary)
                        .width(Length::Fixed(60.0)),
                )
                .push(if balance_masked {
                    Row::new().push(text("********").size(P1_SIZE))
                } else if usdt_balance_error {
                    Row::new().push(text("Balance unavailable").size(P1_SIZE).color(color::RED))
                } else {
                    Row::new()
                        .spacing(6)
                        .align_y(Alignment::Center)
                        .push(text(format_usdt_display(usdt_balance)).size(P1_SIZE).bold())
                        .push(text("USDt").size(P2_SIZE).color(color::GREY_3))
                })
                .push(Space::new().width(Length::Fill))
                .push(
                    button::primary(None, "Send")
                        .width(Length::Fixed(90.0))
                        .on_press(Message::Home(HomeMessage::SendAsset(
                            crate::app::state::liquid::send::SendAsset::Usdt,
                        ))),
                )
                .push(orange_outline_btn(
                    "Receive",
                    Message::Home(HomeMessage::ReceiveAsset(
                        crate::app::state::liquid::send::SendAsset::Usdt,
                    )),
                )),
        )
        .push_maybe(
            (!balance_masked && !usdt_balance_error && pending_usdt_send_sats > 0).then(|| {
                Row::new()
                    .spacing(6)
                    .align_y(Alignment::Center)
                    .push(warning_icon().size(12).style(theme::text::secondary))
                    .push(
                        text(format!(
                            "-{} USDt pending",
                            format_usdt_display(pending_usdt_send_sats)
                        ))
                        .size(P2_SIZE)
                        .style(theme::text::secondary),
                    )
            }),
        )
        .push_maybe(
            (!balance_masked && !usdt_balance_error && pending_usdt_receive_sats > 0).then(|| {
                Row::new()
                    .spacing(6)
                    .align_y(Alignment::Center)
                    .push(warning_icon().size(12).style(theme::text::secondary))
                    .push(
                        text(format!(
                            "+{} USDt pending",
                            format_usdt_display(pending_usdt_receive_sats)
                        ))
                        .size(P2_SIZE)
                        .style(theme::text::secondary),
                    )
            }),
        );

    let liquid_card_content = Column::new()
        .spacing(12)
        .push(
            Row::new()
                .spacing(8)
                .align_y(Alignment::Center)
                .push(droplet_fill_icon().size(16).style(theme::text::secondary))
                .push(text("Liquid").size(14).style(theme::text::secondary)),
        )
        .push(
            Column::new()
                .spacing(4)
                .push(if balance_masked {
                    Row::new().push(text("********").size(H2_SIZE))
                } else {
                    amount_with_size_and_unit(&total_amount, H2_SIZE, bitcoin_unit)
                })
                .push(if balance_masked {
                    Some(text("********").size(P1_SIZE))
                } else {
                    total_fiat.map(|f| f.to_text().size(P1_SIZE).style(theme::text::secondary))
                }),
        )
        .push(lbtc_row)
        .push(usdt_row);

    let liquid_card: Element<'a, Message> = Container::new(liquid_card_content)
        .padding(20)
        .style(|t| iced::widget::container::Style {
            border: iced::Border {
                color: color::ORANGE,
                width: 0.2,
                radius: 25.0.into(),
            },
            background: Some(iced::Background::Color(t.colors.cards.simple.background)),
            ..Default::default()
        })
        .into();

    let liquid_card =
        mouse_area(liquid_card).on_press(Message::Menu(Menu::Liquid(LiquidSubMenu::Overview)));

    // --- Spark card (BTC row only, mirrors the Liquid layout) ---
    // Rendered above the Liquid card because Spark is the default
    // wallet for everyday Lightning UX post-Phase 5. The card only
    // surfaces when `has_spark` is true — cubes without a Spark
    // signer hide it entirely.
    let spark_fiat = fiat_converter.as_ref().map(|c| c.convert(spark_balance));
    let spark_btc_row = Row::new()
        .spacing(8)
        .align_y(Alignment::Center)
        .push(coincube_ui::image::asset_network_logo::<Message>(
            "btc", "spark", 28.0,
        ))
        .push(
            text("BTC")
                .size(P1_SIZE)
                .style(theme::text::secondary)
                .width(Length::Fixed(60.0)),
        )
        .push(if balance_masked {
            Row::new().push(text("********").size(P1_SIZE))
        } else {
            amount_with_size_and_unit(&spark_balance, P1_SIZE, bitcoin_unit)
        })
        .push_maybe(
            (!balance_masked)
                .then(|| {
                    spark_fiat
                        .as_ref()
                        .map(|f| f.to_text().size(P2_SIZE).style(theme::text::secondary))
                })
                .flatten(),
        )
        .push(Space::new().width(Length::Fill))
        .push(
            button::primary(None, "Send")
                .width(Length::Fixed(90.0))
                .on_press(Message::Home(HomeMessage::SendSparkBtc)),
        )
        .push(orange_outline_btn(
            "Receive",
            Message::Home(HomeMessage::ReceiveSparkBtc),
        ));

    let spark_card_content = Column::new()
        .spacing(12)
        .push(
            Row::new()
                .spacing(8)
                .align_y(Alignment::Center)
                .push(lightning_icon().size(16).style(theme::text::secondary))
                .push(text("Spark").size(14).style(theme::text::secondary)),
        )
        .push(
            Column::new()
                .spacing(4)
                .push(if balance_masked {
                    Row::new().push(text("********").size(H2_SIZE))
                } else {
                    amount_with_size_and_unit(&spark_balance, H2_SIZE, bitcoin_unit)
                })
                .push(if balance_masked {
                    Some(text("********").size(P1_SIZE))
                } else {
                    spark_fiat
                        .as_ref()
                        .map(|f| f.to_text().size(P1_SIZE).style(theme::text::secondary))
                }),
        )
        .push(spark_btc_row)
        .push_maybe(pending_spark_incoming.and_then(|pt| {
            (pt.stage != TransferStage::Completed).then(|| pending_deposit_card(pt, bitcoin_unit))
        }));

    let spark_card: Element<'a, Message> = Container::new(spark_card_content)
        .padding(20)
        .style(|t| iced::widget::container::Style {
            border: iced::Border {
                color: color::ORANGE,
                width: 0.2,
                radius: 25.0.into(),
            },
            background: Some(iced::Background::Color(t.colors.cards.simple.background)),
            ..Default::default()
        })
        .into();

    let spark_card = mouse_area(spark_card).on_press(Message::Menu(Menu::Spark(
        crate::app::menu::SparkSubMenu::Overview,
    )));

    let vault_card_element = mouse_area(wallet_card(
        WalletType::Vault,
        &vault_balance,
        fiat_converter,
        balance_masked,
        has_vault,
        bitcoin_unit,
        pending_vault_incoming,
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
        .push_maybe(has_spark.then_some(spark_card))
        .push(liquid_card)
        .push(vault_card_element)
        .push_maybe(transfer_available(has_vault, has_spark).then(|| {
            Container::new(
                button::secondary(Some(arrow_down_up_icon()), "Transfer")
                    .style(|t, _s| iced::widget::button::Style {
                        text_color: color::ORANGE,
                        border: iced::Border {
                            color: color::ORANGE,
                            width: 1.0,
                            radius: 35.0.into(),
                        },
                        background: Some(iced::Background::Color(t.colors.cards.simple.background)),
                        ..Default::default()
                    })
                    .width(Length::Fixed(150.0))
                    .on_press(Message::Home(HomeMessage::NextStep)),
            )
            .width(Length::Fill)
            .center_x(Length::Fill)
        }))
        .into()
}

/// Gate for the page-level Transfer button. Liquid is always present on every
/// cube, so transferring is only possible when at least one *other* wallet is
/// available — Vault (a separate signer) or Spark (a separate bridge-backed
/// wallet).
fn transfer_available(has_vault: bool, has_spark: bool) -> bool {
    has_vault || has_spark
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every directed pair of distinct wallets must map to exactly one
    /// `TransferDirection`, and same-wallet pairs must map to `None`.
    #[test]
    fn try_from_pair_is_total_over_distinct_pairs() {
        let kinds = [WalletKind::Liquid, WalletKind::Spark, WalletKind::Vault];
        for from in kinds {
            for to in kinds {
                match TransferDirection::try_from_pair(from, to) {
                    Some(direction) => {
                        assert_ne!(from, to, "distinct pair expected for {from:?}→{to:?}");
                        assert_eq!(direction.pair(), (from, to));
                    }
                    None => {
                        assert_eq!(
                            from, to,
                            "try_from_pair returned None for distinct pair {from:?}→{to:?}"
                        );
                    }
                }
            }
        }
    }

    /// Round-trip: every variant's `pair()` reconstructs the same variant.
    #[test]
    fn pair_roundtrip_for_all_variants() {
        use TransferDirection::*;
        for direction in [
            LiquidToVault,
            VaultToLiquid,
            LiquidToSpark,
            SparkToLiquid,
            VaultToSpark,
            SparkToVault,
        ] {
            let (from, to) = direction.pair();
            assert_eq!(
                TransferDirection::try_from_pair(from, to),
                Some(direction),
                "roundtrip failed for {direction:?}"
            );
        }
    }

    /// `from_kind()`/`to_kind()` must agree with `pair()`.
    #[test]
    fn from_to_kind_agrees_with_pair() {
        use TransferDirection::*;
        for direction in [
            LiquidToVault,
            VaultToLiquid,
            LiquidToSpark,
            SparkToLiquid,
            VaultToSpark,
            SparkToVault,
        ] {
            let (from, to) = direction.pair();
            assert_eq!(direction.from_kind(), from);
            assert_eq!(direction.to_kind(), to);
        }
    }

    /// Picker rows render a network badge — the uppercase label must be
    /// stable across changes (copy contract with the Liquid Send picker).
    #[test]
    fn wallet_kind_badges_are_uppercase() {
        assert_eq!(WalletKind::Liquid.badge(), "LIQUID");
        assert_eq!(WalletKind::Spark.badge(), "SPARK");
        assert_eq!(WalletKind::Vault.badge(), "VAULT");
    }

    /// The page-level Transfer button is only visible when at least one
    /// non-Liquid wallet exists (Liquid alone has nothing to transfer with).
    #[test]
    fn transfer_available_gate() {
        assert!(!transfer_available(false, false));
        assert!(transfer_available(true, false));
        assert!(transfer_available(false, true));
        assert!(transfer_available(true, true));
    }
}

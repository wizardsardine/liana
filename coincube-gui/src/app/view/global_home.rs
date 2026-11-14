use breez_sdk_liquid::{bitcoin::Denomination, model::PreparePayOnchainResponse};
use coincube_ui::locale::copy as localized_copy;
use coincube_ui::locale::Text as CopyText;
use coincube_ui::localized_format;
use std::time::Duration;

use coincube_ui::component::text::Text;
use coincube_ui::{
    color,
    component::{amount::*, button, form, spinner, text::*},
    font::MEDIUM,
    icon::{
        arrow_right, check_circle_icon, clipboard_icon, clock_icon, dash_circle_icon,
        droplet_fill_icon, exclamation_circle_icon, eye_icon, eye_slash_icon, key_icon, lock_icon,
        spark_icon, usd_icon, vault_icon, warning_icon,
    },
    theme,
    widget::{action::Action, *},
};
use iced::{
    widget::{Button, Column, Space},
    Alignment, Length,
};

use crate::app::breez_liquid::assets::format_usdt_display;
use crate::app::transfer_destination_loader::CubeChoice;
use crate::app::view::shared::landing::{
    amount_cell, landing_button, landing_card_style, landing_control_id, AmountCell, AmountTier,
};
use crate::app::view::wallet_header::{wallet_header, HeaderVariant, SyncState, WalletHeaderProps};
use crate::app::{menu::Menu, view::FiatAmountConverter};
use crate::app::{
    menu::{LiquidSubMenu, VaultSubMenu},
    view::message::{HomeMessage, Message},
};
use crate::chain::ChainId;
use coincube_core::miniscript::bitcoin::Amount;

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
/// the Done page (`done_page`) renders the "Pending Deposit"
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
    balance_masked: bool,
) -> Element<'a, Message> {
    let display_amount = pending_deposit_amount(pending_transfer, bitcoin_unit, balance_masked);
    Container::new(
        Row::new()
            .spacing(8)
            .align_y(Alignment::Center)
            .push(warning_icon().size(12).style(theme::text::secondary))
            .push(
                text(localized_copy("Pending deposit"))
                    .size(12)
                    .style(theme::text::secondary),
            )
            .push(Space::new().width(Length::Fill))
            .push(text(display_amount).size(12).color(color::ORANGE)),
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

fn pending_deposit_amount(
    pending_transfer: PendingTransfer,
    bitcoin_unit: BitcoinDisplayUnit,
    balance_masked: bool,
) -> String {
    if balance_masked {
        AmountCell::masked().primary
    } else {
        format!(
            "{} {}",
            pending_transfer
                .amount
                .to_formatted_string_with_unit(bitcoin_unit),
            bitcoin_unit.label()
        )
    }
}

#[allow(clippy::too_many_arguments)]
fn wallet_card<'a>(
    wallet_type: WalletType,
    cube_id: &str,
    balance: &Amount,
    fiat_converter: Option<FiatAmountConverter>,
    balance_masked: bool,
    has_vault: bool,
    show_recovery_path: bool,
    import_pending: bool,
    bitcoin_unit: coincube_ui::component::amount::BitcoinDisplayUnit,
    pending_vault_incoming: Option<PendingTransfer>,
    pending_send_sats: u64,
    pending_receive_sats: u64,
    display_mode: crate::app::settings::display::DisplayMode,
    // True once the underlying wallet has reported its first balance.
    // Drives the animated-dots loading placeholder and suppresses
    // pending indicators that would otherwise read alongside a
    // not-yet-authoritative value.
    loaded: bool,
    vault_alias: Option<&str>,
    // The Vault's stale-balance line while it isn't updating.
    stale: Option<String>,
) -> Element<'a, Message> {
    let fiat_balance = fiat_converter.as_ref().map(|c| c.convert(*balance));

    let (icon, title, send_action, receive_action) = match wallet_type {
        WalletType::Liquid => (
            droplet_fill_icon().style(theme::text::secondary),
            "Liquid",
            Message::Menu(Menu::Liquid(LiquidSubMenu::Send)),
            Message::Menu(Menu::Liquid(LiquidSubMenu::Receive)),
        ),
        WalletType::Usdt { .. } => (
            usd_icon().style(theme::text::secondary),
            "USDt",
            Message::Menu(Menu::Liquid(LiquidSubMenu::Send)),
            Message::Menu(Menu::Liquid(LiquidSubMenu::Receive)),
        ),
        WalletType::Vault => (
            vault_icon().style(theme::text::secondary),
            "Vault",
            Message::Menu(Menu::Vault(VaultSubMenu::Send)),
            Message::Menu(Menu::Vault(VaultSubMenu::Receive)),
        ),
    };
    let overview_action = match wallet_type {
        WalletType::Vault => Message::Menu(Menu::Vault(VaultSubMenu::Overview)),
        WalletType::Liquid | WalletType::Usdt { .. } => {
            Message::Menu(Menu::Liquid(LiquidSubMenu::Overview))
        }
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
                                    text(localized_copy("Balance unavailable"))
                                        .size(P1_SIZE)
                                        .color(color::RED),
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
                                                text(localized_format!(
                                                    "-{} USDt pending",
                                                    "-{} USDt pendiente",
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
                                                text(localized_format!(
                                                    "+{} USDt pending",
                                                    "+{} USDt pendiente",
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
                        button::primary(None, localized_copy("Send"))
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
        return openable_card(
            Container::new(content)
                .padding(20)
                .style(landing_card_style),
            overview_action,
        );
    }

    let content = match wallet_type {
        WalletType::Vault if import_pending => Column::new().spacing(12).push(
            Row::new()
                .spacing(8)
                .align_y(Alignment::Center)
                .push(landing_button(
                    landing_control_id(cube_id, "Vault", "open"),
                    "Vault",
                    Some(vault_icon()),
                    Message::FocusAction,
                    Some(overview_action.clone()),
                    theme::button::transparent,
                ))
                .push(text(localized_copy("Wallet import pending")).style(theme::text::secondary))
                .push(Space::new().width(Length::Fill))
                .push(landing_button(
                    landing_control_id(cube_id, "Vault", "finish-import"),
                    localized_copy("Finish import"),
                    None,
                    Message::FocusAction,
                    Some(Message::Menu(Menu::Vault(VaultSubMenu::Overview))),
                    theme::button::primary,
                )),
        ),
        WalletType::Vault if !has_vault => Column::new().spacing(12).push(
            Row::new()
                .spacing(8)
                .align_y(Alignment::Center)
                .push(landing_button(
                    landing_control_id(cube_id, "Vault", "open"),
                    "Vault",
                    Some(vault_icon()),
                    Message::FocusAction,
                    Some(overview_action.clone()),
                    theme::button::transparent,
                ))
                .push(
                    Row::new()
                        .spacing(10)
                        .align_y(Alignment::Center)
                        .push(Space::new().width(Length::Fill))
                        .push(
                            landing_button(
                                landing_control_id(cube_id, "Vault", "restore"),
                                localized_copy("Restore from Connect"),
                                None,
                                Message::FocusAction,
                                Some(Message::SetupVaultRestoreFromKit),
                                theme::button::secondary,
                            )
                            .width(Length::Fixed(200.0)),
                        )
                        .push(
                            landing_button(
                                landing_control_id(cube_id, "Vault", "create"),
                                localized_copy("Create Vault"),
                                None,
                                Message::FocusAction,
                                Some(Message::SetupVault),
                                theme::button::primary,
                            )
                            .width(Length::Fixed(160.0)),
                        ),
                ),
        ),
        _ => {
            // BTC currency row mirrors Spark's BTC row / Liquid's L-BTC row,
            // so Vault's Send/Receive lives at the row level rather than as
            // duplicate card-level buttons. Vault's send/receive actions
            // route through Menu navigation (`send_action`/`receive_action`).
            let btc_fiat_text =
                fiat_balance.map(|f| f.to_text().size(P2_SIZE).style(theme::text::secondary));
            // `fit_row`: the actions drop below the amounts in a card too
            // narrow for both, rather than being squeezed out of it.
            let btc_row = coincube_ui::component::tenshu::fit_row(
                Row::new()
                    .spacing(8)
                    .align_y(Alignment::Center)
                    .push(coincube_ui::image::asset_network_logo::<Message>(
                        "btc", "bitcoin", 32.0,
                    ))
                    .push(text("BTC").size(P1_SIZE).bold().width(Length::Fixed(60.0)))
                    .push(if balance_masked {
                        Row::new().push(text("********").size(P1_SIZE))
                    } else if !loaded {
                        Row::new().push(spinner::typing_text_carousel(
                            "...",
                            false,
                            Duration::from_millis(400),
                            |content| text(content).size(P1_SIZE).style(theme::text::secondary),
                        ))
                    } else {
                        amount_with_size_and_unit(balance, P1_SIZE, bitcoin_unit)
                    })
                    .push_maybe(
                        (!balance_masked && loaded)
                            .then_some(btc_fiat_text)
                            .flatten(),
                    ),
                Row::new()
                    .spacing(8)
                    .align_y(Alignment::Center)
                    .push(
                        landing_button(
                            landing_control_id(cube_id, title, "send-btc"),
                            localized_copy("Send"),
                            None,
                            Message::FocusAction,
                            Some(send_action),
                            theme::button::primary,
                        )
                        .width(Length::Fixed(90.0)),
                    )
                    .push(
                        landing_button(
                            landing_control_id(cube_id, title, "receive-btc"),
                            localized_copy("Receive"),
                            None,
                            Message::FocusAction,
                            Some(receive_action),
                            theme::button::orange_outline,
                        )
                        .width(Length::Fixed(90.0)),
                    ),
            );

            // Headline — animated dots while the daemon hasn't
            // reported a first balance. Pending send/receive counts
            // are zeroed in the loading branch so the wallet_header's
            // own pending indicators stay suppressed alongside the
            // placeholder.
            let header: Element<'a, Message> = if loaded {
                wallet_header::<Message>(WalletHeaderProps {
                    sats: *balance,
                    fiat: fiat_balance,
                    balance_masked,
                    bitcoin_unit,
                    variant: HeaderVariant::Card,
                    sync: SyncState::Synced,
                    unconfirmed: None,
                    pending_send_sats,
                    pending_receive_sats,
                    display_mode,
                    on_swap: Some(Message::FlipDisplayMode),
                    on_swap_focus: Some((
                        landing_control_id(cube_id, title, "flip-balance"),
                        Message::FocusAction,
                    )),
                })
                .into()
            } else {
                Column::new()
                    .spacing(4)
                    .push(spinner::typing_text_carousel(
                        "...",
                        false,
                        Duration::from_millis(400),
                        |content| text(content).size(H2_SIZE).bold(),
                    ))
                    .push(spinner::typing_text_carousel(
                        "...",
                        false,
                        Duration::from_millis(400),
                        |content| text(content).size(P1_SIZE).style(theme::text::secondary),
                    ))
                    .into()
            };

            let title_label = if matches!(wallet_type, WalletType::Vault) {
                vault_alias
                    .filter(|alias| !alias.trim().is_empty())
                    .map(|alias| format!("Vault · {alias}"))
                    .unwrap_or_else(|| title.to_owned())
            } else {
                title.to_owned()
            };
            let title_action = Action::new(
                landing_control_id(cube_id, title, "open"),
                Container::new(
                    Row::new()
                        .spacing(10)
                        .align_y(Alignment::Center)
                        .push(icon.size(16))
                        .push(text(title_label).font(MEDIUM)),
                )
                .padding(5),
                Message::FocusAction,
            )
            .style(theme::button::transparent)
            .on_press(overview_action.clone());
            // Like Spark's "Last known Spark balance · refresh failed".
            let stale = stale.map(|line| {
                Row::new()
                    .spacing(6)
                    .align_y(Alignment::Center)
                    .push(warning_icon().size(14).style(theme::text::warning))
                    .push(text(line).size(P2_SIZE).style(theme::text::warning))
            });
            let content = Column::new()
                .spacing(12)
                .push(title_action)
                .push(header)
                .push_maybe(stale)
                .push(btc_row);

            // Open the available recovery paths directly in Tools.
            if show_recovery_path {
                content.push(
                    Column::new()
                        .spacing(10)
                        .push(iced::widget::rule::horizontal(1))
                        .push(
                            Action::new(
                                landing_control_id(cube_id, "Vault", "recovery-path"),
                                Container::new(
                                    Row::new()
                                        .spacing(6)
                                        .align_y(Alignment::Center)
                                        .push(clock_icon().size(16))
                                        .push(
                                            text(localized_copy("Recovery path"))
                                                .size(P1_SIZE)
                                                .bold(),
                                        )
                                        .push(arrow_right().size(14)),
                                )
                                .padding(4),
                                Message::FocusAction,
                            )
                            .style(|t, status| {
                                let mut style = theme::button::link(t, status);
                                style.text_color = t.colors.shell.accent_ink;
                                style
                            })
                            .on_press(Message::Menu(Menu::Vault(VaultSubMenu::Recovery))),
                        ),
                )
            } else {
                content
            }
        }
    };

    let content = if matches!(wallet_type, WalletType::Vault) && loaded {
        if let Some(pending_transfer) = pending_vault_incoming {
            if pending_transfer.stage != TransferStage::Completed {
                content.push(pending_deposit_card(
                    pending_transfer,
                    bitcoin_unit,
                    balance_masked,
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

    openable_card(
        Container::new(content)
            .padding(20)
            .style(landing_card_style),
        overview_action,
    )
}

/// A wallet card opens its wallet from anywhere on its surface, not only
/// from its title. The buttons, links and balance swap inside it capture
/// their own presses, so this fires only where nothing else would. The
/// title stays the card's focusable control for keyboard users.
fn openable_card<'a>(card: impl Into<Element<'a, Message>>, open: Message) -> Element<'a, Message> {
    iced::widget::mouse_area(card)
        .on_press(open)
        .interaction(iced::mouse::Interaction::Pointer)
        .into()
}

/// Steps of a transfer within this Cube (P-Transfer `pathSteps`).
/// Steps of every transfer: one Details card, then Review, then Sent.
const TRANSFER_STEPS: [&str; 3] = ["Details", "Review", "Sent"];

/// Which page `HomeView` step 1 shows. Review and Done stay steps 2 and 3.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TransferPage {
    #[default]
    Wallets,
    OtherCube,
    Unlock,
    Amount,
}

/// Number of checks the other Cube's unlock runs: credential, Vault
/// identity, receive address.
pub const UNLOCK_CHECKS: u8 = 3;

/// Progress of the other Cube's in-place unlock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnlockPhase {
    /// Waiting for the PIN or passkey; `busy` while it is being checked.
    Credential { busy: bool },
    /// Checks passed so far, out of [`UNLOCK_CHECKS`].
    Checks(u8),
}

impl Default for UnlockPhase {
    fn default() -> Self {
        Self::Credential { busy: false }
    }
}

/// What the "Another Cube…" route shows. Built from the state's catalog
/// snapshot; no balance of a locked Cube is ever read.
#[derive(Clone, Copy, Debug, Default)]
pub struct OtherCubeView<'a> {
    pub choices: &'a [CubeChoice],
    pub selected: Option<&'a str>,
    pub pin: Option<&'a crate::pin_input::PinInput>,
    pub phase: UnlockPhase,
    pub error: Option<&'a str>,
    pub spark_address: Option<&'a str>,
    /// The destination Cube is unlocked for this transfer.
    pub unlocked: bool,
    /// The transfer was sent and the destination Cube locked again.
    pub locked_again: bool,
}

impl<'a> OtherCubeView<'a> {
    fn choice(&self) -> Option<&'a CubeChoice> {
        let selected = self.selected?;
        self.choices
            .iter()
            .find(|choice| choice.cube_id == selected)
    }

    fn name(&self) -> &'a str {
        self.choice()
            .map_or("the other Cube", |choice| choice.name.as_str())
    }
}

/// Everything the dialog's pages share.
#[derive(Clone, Copy)]
struct TransferCtx<'a> {
    cube_id: &'a str,
    cube_name: &'a str,
    chain: ChainId,
    direction: TransferDirection,
    has_vault: bool,
    has_spark: bool,
    has_liquid: bool,
    liquid_balance: Amount,
    spark_balance: Amount,
    vault_balance: Amount,
    fiat_converter: Option<FiatAmountConverter>,
    unit: BitcoinDisplayUnit,
    max: bool,
    balance_masked: bool,
    other: OtherCubeView<'a>,
    /// The route tile whose menu is open.
    menu: Option<PickerSide>,
    /// The wallet page leaving returns to; `None` for the Overview.
    origin: Option<WalletKind>,
}

impl TransferCtx<'_> {
    fn balance(&self, kind: WalletKind) -> Amount {
        match kind {
            WalletKind::Liquid => self.liquid_balance,
            WalletKind::Spark => self.spark_balance,
            WalletKind::Vault => self.vault_balance,
        }
    }

    fn steps(&self) -> &'static [&'static str] {
        &TRANSFER_STEPS
    }

    fn review_index(&self) -> usize {
        1
    }
}

fn unit_label(unit: BitcoinDisplayUnit) -> &'static str {
    match unit {
        BitcoinDisplayUnit::BTC => "BTC",
        BitcoinDisplayUnit::Sats => "SATS",
    }
}

/// "25,053 SATS", or the mask.
fn bitcoin_text(amount: Amount, unit: BitcoinDisplayUnit, masked: bool) -> String {
    if masked {
        "••••••".to_string()
    } else {
        format!(
            "{} {}",
            amount.to_formatted_string_with_unit(unit),
            unit_label(unit)
        )
    }
}

fn muted<'a>(content: impl iced::widget::text::IntoFragment<'a>) -> coincube_ui::widget::Text<'a> {
    text(content).size(13).style(theme::text::secondary)
}

/// P-Transfer's `.grp`: a group after the first gets a rule above it.
fn group<'a>(
    legend: Option<String>,
    body: impl Into<Element<'a, Message>>,
) -> Element<'a, Message> {
    Column::new()
        .spacing(8)
        .push_maybe(legend.map(|legend| text(legend).size(14).bold()))
        .push(body)
        .into()
}

/// P-Transfer's `.acts`: secondary on the left, primary on the right.
fn page_actions<'a>(
    back: Option<Element<'a, Message>>,
    primary: Element<'a, Message>,
) -> Element<'a, Message> {
    Row::new()
        .spacing(10)
        .align_y(Alignment::Center)
        .push_maybe(back)
        .push(Space::new().width(Length::Fill))
        .push(primary)
        .into()
}

/// P-Transfer's `.btn`: 36px tall, sized to its label.
fn dialog_button<'a>(
    label: impl Into<String>,
    icon: Option<coincube_ui::widget::Text<'a>>,
    primary: bool,
    on_press: Option<Message>,
) -> Element<'a, Message> {
    Button::new(
        Row::new()
            .spacing(6)
            .align_y(Alignment::Center)
            .push_maybe(icon.map(|icon| icon.size(14)))
            .push(text(label.into()).bold()),
    )
    .padding([8, 16])
    .style(if primary {
        theme::button::primary
    } else {
        theme::button::secondary
    })
    .on_press_maybe(on_press)
    .into()
}

fn back_button<'a>(label: &'static str, on_press: Option<Message>) -> Element<'a, Message> {
    dialog_button(label, None, false, on_press)
}

fn status_line<'a>(
    icon: coincube_ui::widget::Text<'a>,
    tone: StatusTone,
    content: String,
) -> Element<'a, Message> {
    let color = move |t: &theme::Theme| iced::widget::text::Style {
        color: Some(match tone {
            StatusTone::Good => t.colors.shell.good,
            StatusTone::Warn => t.colors.shell.warn,
            StatusTone::Bad => t.colors.shell.bad,
            StatusTone::Muted => t.colors.shell.muted,
        }),
    };
    Row::new()
        .spacing(6)
        .align_y(Alignment::Center)
        .push(icon.size(14).style(color))
        .push(text(content).size(13).style(color))
        .into()
}

#[derive(Clone, Copy)]
enum StatusTone {
    Good,
    Warn,
    Bad,
    Muted,
}

fn network_name(chain: ChainId) -> &'static str {
    crate::app::view::nav::shell::network_label(chain)
}

/// The persisted top-bar preference is the privacy floor for both Overview sections.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct OverviewMasks {
    total: bool,
    wallets: bool,
}

impl OverviewMasks {
    fn new(global: bool, total: bool, wallets: bool) -> Self {
        Self {
            total: global || total,
            wallets: global || wallets,
        }
    }
}

fn overview_eye<'a>(
    cube_id: &str,
    section: &'static str,
    masked: bool,
    global_hidden: bool,
    toggle: HomeMessage,
) -> Element<'a, Message> {
    let glyph = if masked { eye_slash_icon() } else { eye_icon() };
    let label = match (section, global_hidden, masked) {
        ("Total Balance", true, _) => localized_copy("Total balance hidden by global Hide"),
        ("Wallets", true, _) => localized_copy("Wallet balances hidden by global Hide"),
        ("Total Balance", false, true) => localized_copy("Show total balance"),
        ("Wallets", false, true) => localized_copy("Show wallet balances"),
        ("Total Balance", false, false) => localized_copy("Hide total balance"),
        _ => localized_copy("Hide wallet balances"),
    };
    iced::widget::tooltip(
        Action::new(
            landing_control_id(cube_id, section, "balance-privacy"),
            glyph.size(16),
            Message::FocusAction,
        )
        .padding([7, 9])
        .style(theme::button::shell_nav)
        .on_press_maybe((!global_hidden).then_some(Message::Home(toggle))),
        Container::new(text(label).size(P2_SIZE))
            .padding([6, 10])
            .style(theme::container::shell_menu),
        iced::widget::tooltip::Position::Bottom,
    )
    .into()
}

/// Outlined panel for the review address.
fn review_panel_style(t: &theme::Theme) -> iced::widget::container::Style {
    iced::widget::container::Style {
        border: iced::Border {
            color: t.colors.shell.line,
            width: 1.0,
            radius: 8.0.into(),
        },
        ..Default::default()
    }
}

/// Review and Sent's card: the form on the panel surface.
fn legacy_form<'a>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    Container::new(content)
        .padding(24)
        .width(Length::Fill)
        .style(crate::app::view::shared::payment_columns::rail_card_style)
        .into()
}

/// The other Cube's state above the form: open while the transfer runs,
/// locked again once it is sent.
fn other_cube_context<'a>(
    ctx: &TransferCtx<'a>,
    open: bool,
    done: bool,
) -> Option<Element<'a, Message>> {
    if !ctx.direction.is_other_cube() {
        return None;
    }
    if done && ctx.other.locked_again {
        return Some(status_line(
            lock_icon(),
            StatusTone::Good,
            localized_format!(
                "{} is locked again.",
                "{} está bloqueada de nuevo.",
                ctx.other.name()
            ),
        ));
    }
    (open && ctx.other.unlocked).then(|| {
        status_line(
            lock_icon(),
            StatusTone::Warn,
            localized_format!(
                "{} · {} · unlocked for this transfer",
                "{} · {} · desbloqueado para esta transferencia",
                ctx.other.name(),
                network_name(ctx.chain)
            ),
        )
    })
}

/// A transfer step as a page of its own (Send's layout): the body under
/// a page head the state layer builds from `step` and `can_leave`.
pub struct TransferScreen<'a> {
    /// The active step on [`TRANSFER_STEPS`]; `TRANSFER_STEPS.len()` once
    /// sent (every step done).
    pub step: usize,
    /// Back is offered; `false` while a send is in flight.
    pub can_leave: bool,
    /// Where Back goes: the wallet page the transfer opened from, or the
    /// Overview.
    pub back_to: &'static str,
    pub body: Element<'a, Message>,
}

/// The transfer as a page: the other Cube's state, then the step's card
/// and rail. The title, Cube line, steps and Back to Overview are the page
/// head's ([`transfer_page_head`]).
fn transfer_dialog<'a>(
    ctx: &TransferCtx<'a>,
    active: usize,
    context: Option<Element<'a, Message>>,
    close: Option<Message>,
    form: impl Into<Element<'a, Message>>,
) -> TransferScreen<'a> {
    TransferScreen {
        step: active,
        can_leave: close.is_some(),
        back_to: back_to(ctx.origin),
        body: Column::new()
            .spacing(16)
            .push_maybe(context)
            .push(form)
            .into(),
    }
}

/// The page Back returns to, named as the side menu names it.
pub fn back_to(origin: Option<WalletKind>) -> &'static str {
    origin.map_or("Overview", |wallet| wallet.label())
}

/// The steps every transfer takes, for the page head's rule.
pub fn transfer_steps_row<'a>(step: usize) -> Element<'a, Message> {
    coincube_ui::component::tenshu::steps(&TRANSFER_STEPS, step)
}

/// The Transfer page's head: title and Cube line, the steps on its rule,
/// and Back to Overview unless a send is in flight.
pub fn transfer_page_head<'a>(
    cache: &crate::app::cache::Cache,
    screen: &TransferScreen<'a>,
) -> coincube_ui::component::tenshu::PageHead<'a, Message> {
    let head = crate::app::view::page_head(cache, localized_copy("Transfer"))
        .rule(transfer_steps_row(screen.step));
    if screen.can_leave {
        head.back(screen.back_to, Message::Home(HomeMessage::LeaveTransfer))
    } else {
        head
    }
}

/// One row of the unlock checks: done, running or waiting.
fn check_row<'a>(label: String, state: std::cmp::Ordering) -> Element<'a, Message> {
    use std::cmp::Ordering::*;
    let (icon, status): (Element<'a, Message>, &str) = match state {
        Less => (
            check_circle_icon()
                .size(16)
                .style(|t: &theme::Theme| iced::widget::text::Style {
                    color: Some(t.colors.shell.good),
                })
                .into(),
            CopyText::Done.get(),
        ),
        Equal => (
            clock_icon().size(16).style(theme::text::secondary).into(),
            "In progress",
        ),
        Greater => (
            dash_circle_icon()
                .size(16)
                .style(theme::text::secondary)
                .into(),
            "Waiting",
        ),
    };
    Column::new()
        .push(
            Row::new()
                .spacing(10)
                .align_y(Alignment::Center)
                .padding([12, 0])
                .push(icon)
                .push(text(label).width(Length::Fill))
                .push(muted(status)),
        )
        .push(iced::widget::rule::horizontal(1))
        .into()
}

fn parse_amount(value: &form::Value<String>, unit: BitcoinDisplayUnit) -> Option<Amount> {
    let denomination = match unit {
        BitcoinDisplayUnit::BTC => Denomination::Bitcoin,
        BitcoinDisplayUnit::Sats => Denomination::Satoshi,
    };
    (!value.value.is_empty())
        .then(|| Amount::from_str_in(&value.value, denomination).ok())
        .flatten()
}

fn fiat_estimate(amount: Amount, fiat_converter: Option<FiatAmountConverter>) -> Option<String> {
    fiat_converter.map(|converter| {
        let fiat = converter.convert(amount);
        format!("≈ {} {}", fiat.to_formatted_string(), fiat.currency())
    })
}

/// Under the amount while Max is on.
const MAX_NOTE: &str = "Sends the whole balance. Fees come out of this amount.";

/// The route tiles' focus ids.
const TRANSFER_FROM_TILE_ID: &str = "transfer-from-tile";
const TRANSFER_TO_TILE_ID: &str = "transfer-to-tile";

/// The wallets this Cube offers, in menu order. Liquid only where the Cube
/// still has a usable Liquid wallet.
fn cube_wallets(ctx: &TransferCtx<'_>) -> Vec<WalletKind> {
    [
        (WalletKind::Spark, ctx.has_spark),
        (WalletKind::Vault, ctx.has_vault),
        (WalletKind::Liquid, ctx.has_liquid),
    ]
    .iter()
    .filter_map(|&(kind, available)| available.then_some(kind))
    .collect()
}

/// A wallet's logo: bitcoin, badged with where it lives.
fn wallet_logo<'a>(kind: WalletKind, size: f32) -> Element<'a, Message> {
    coincube_ui::image::asset_network_logo(
        "btc",
        match kind {
            WalletKind::Spark => "spark",
            WalletKind::Vault => "bitcoin",
            WalletKind::Liquid => "liquid",
        },
        size,
    )
}

fn from_option_id(kind: WalletKind) -> &'static str {
    match kind {
        WalletKind::Spark => "transfer-from-spark",
        WalletKind::Vault => "transfer-from-vault",
        WalletKind::Liquid => "transfer-from-liquid",
    }
}

fn to_option_id(kind: WalletKind) -> &'static str {
    match kind {
        WalletKind::Spark => "transfer-to-spark",
        WalletKind::Vault => "transfer-to-vault",
        WalletKind::Liquid => "transfer-to-liquid",
    }
}

/// The From menu: this Cube's wallets with what each can send. An empty
/// wallet can't be picked.
fn from_menu<'a>(ctx: &TransferCtx<'a>) -> Element<'a, Message> {
    use crate::app::view::shared::picker::{
        dropdown_heading, dropdown_menu, dropdown_option, DROPDOWN_LOGO,
    };
    let source = ctx.direction.from_kind();
    let mut list = coincube_ui::widget::Column::new().push(dropdown_heading(&localized_format!(
        "Wallets in {}",
        "Billeteras en {}",
        ctx.cube_name
    )));
    for kind in cube_wallets(ctx) {
        let balance = ctx.balance(kind);
        let empty = balance == Amount::ZERO && !ctx.balance_masked;
        list = list.push(dropdown_option(
            from_option_id(kind),
            wallet_logo(kind, DROPDOWN_LOGO),
            kind.label(),
            "BTC",
            Some(if empty {
                localized_copy("Empty · nothing to transfer").to_string()
            } else {
                localized_format!(
                    "Available {}",
                    "{} disponible",
                    bitcoin_text(balance, ctx.unit, ctx.balance_masked)
                )
            }),
            kind == source,
            (!empty).then_some(Message::Home(HomeMessage::SelectTransferWallet(
                PickerSide::From,
                kind,
            ))),
            Message::FocusAction,
        ));
    }
    dropdown_menu(list)
}

/// The To menu: this Cube's other wallets, then each wallet another local
/// Cube on this network can receive into. Locked Cubes show no balances;
/// one that can't receive says why.
fn to_menu<'a>(ctx: &TransferCtx<'a>) -> Element<'a, Message> {
    use crate::app::view::shared::picker::{
        dropdown_heading, dropdown_menu, dropdown_note, dropdown_option, DROPDOWN_LOGO,
    };
    let source = ctx.direction.from_kind();
    let other = ctx.direction.is_other_cube();
    let mut list = coincube_ui::widget::Column::new().push(dropdown_heading(&localized_format!(
        "This Cube · {}",
        "Este Cubo · {}",
        ctx.cube_name
    )));
    for kind in cube_wallets(ctx).into_iter().filter(|kind| *kind != source) {
        list = list.push(dropdown_option(
            to_option_id(kind),
            wallet_logo(kind, DROPDOWN_LOGO),
            kind.label(),
            "BTC",
            Some(localized_format!(
                "Balance {}",
                "Saldo {}",
                bitcoin_text(ctx.balance(kind), ctx.unit, ctx.balance_masked)
            )),
            !other && ctx.direction.to_kind() == kind,
            Some(Message::Home(HomeMessage::SelectTransferWallet(
                PickerSide::To,
                kind,
            ))),
            Message::FocusAction,
        ));
    }
    if TransferDirection::to_other_cube(source, ctx.chain).is_some() {
        list = list.push(dropdown_heading(localized_copy("Other Cubes")));
        if ctx.other.choices.is_empty() {
            list = list.push(dropdown_note(localized_copy(
                "No other Cubes on this device.",
            )));
        }
        for choice in ctx.other.choices {
            let kinds: Vec<WalletKind> = [WalletKind::Spark, WalletKind::Vault]
                .iter()
                .copied()
                .filter(|kind| choice.can_receive(*kind))
                .collect();
            if kinds.is_empty() {
                // One row that says why this Cube can't receive.
                list = list.push(dropdown_option(
                    format!("transfer-to-cube-{}", choice.cube_id),
                    wallet_logo(WalletKind::Spark, DROPDOWN_LOGO),
                    &choice.name,
                    "Can't receive",
                    Some(
                        choice
                            .refusal
                            .map(|reason| reason.to_string())
                            .unwrap_or_else(|| {
                                localized_copy("No wallet that can receive").to_string()
                            }),
                    ),
                    false,
                    None,
                    Message::FocusAction,
                ));
                continue;
            }
            for kind in kinds {
                let selected = other
                    && ctx.other.selected == Some(choice.cube_id.as_str())
                    && ctx.direction.to_kind() == kind;
                list = list.push(dropdown_option(
                    format!("transfer-to-{}-{}", choice.cube_id, kind.label()),
                    wallet_logo(kind, DROPDOWN_LOGO),
                    &localized_format!("{} in {}", "{} en {}", kind.label(), choice.name),
                    if choice.passkey { "Passkey" } else { "PIN" },
                    Some(localized_copy("Locked · unlocks for this transfer").to_string()),
                    selected,
                    Some(Message::Home(HomeMessage::PickOtherCubeWallet(
                        choice.cube_id.clone(),
                        kind,
                    ))),
                    Message::FocusAction,
                ));
            }
        }
        list = list.push(dropdown_note(localized_copy(
            "Locked Cubes show no balances.",
        )));
    }
    dropdown_menu(list)
}

/// A route tile that opens its menu: the Send tile's surface, the accent
/// line while open.
fn route_tile<'a>(
    id: &'static str,
    body: Element<'a, Message>,
    open: bool,
    side: PickerSide,
) -> Element<'a, Message> {
    Action::new(id, body, Message::FocusAction)
        .style(if open {
            crate::app::view::spark::send::open_tile_button_style
        } else {
            crate::app::view::spark::send::receive_tile_button_style
        })
        .padding(0)
        .width(Length::Fill)
        .focus_radius(8.0)
        .on_press(Message::Home(HomeMessage::OpenWalletPicker(side)))
        .into()
}

/// From → To, as Send's YOU SEND → THEY RECEIVE pair: each tile opens its
/// menu under it.
fn transfer_route_tiles<'a>(ctx: &TransferCtx<'a>, page: TransferPage) -> Element<'a, Message> {
    use crate::app::view::shared::picker::anchored_dropdown;
    use crate::app::view::spark::route_tile_body_with_detail;
    let direction = ctx.direction;
    let source = direction.from_kind();
    let from_open = ctx.menu == Some(PickerSide::From);
    let to_open = ctx.menu == Some(PickerSide::To);
    let from = route_tile(
        TRANSFER_FROM_TILE_ID,
        route_tile_body_with_detail(
            wallet_logo(source, 36.0),
            "From",
            None,
            source.label().to_string(),
            localized_format!("in {}", "en {}", ctx.cube_name),
            Some(localized_format!(
                "Available {}",
                "{} disponible",
                bitcoin_text(ctx.balance(source), ctx.unit, ctx.balance_masked)
            )),
            Some(from_open),
        ),
        from_open,
        PickerSide::From,
    );
    let to_body = if !direction.is_other_cube() {
        let kind = direction.to_kind();
        route_tile_body_with_detail(
            wallet_logo(kind, 36.0),
            "To",
            None,
            kind.label().to_string(),
            localized_format!("in {}", "en {}", ctx.cube_name),
            Some(localized_format!(
                "Balance {}",
                "Saldo {}",
                bitcoin_text(ctx.balance(kind), ctx.unit, ctx.balance_masked)
            )),
            Some(to_open),
        )
    } else if let Some(choice) = ctx.other.choice().filter(|_| page != TransferPage::Wallets) {
        let unlocked = page == TransferPage::Amount;
        route_tile_body_with_detail(
            wallet_logo(direction.to_kind(), 36.0),
            "To",
            Some(if unlocked { "Unlocked" } else { "Locked" }),
            direction.to_wallet().to_string(),
            localized_format!("in {}", "en {}", choice.name),
            Some(if unlocked {
                localized_copy("Unlocked for this transfer").to_string()
            } else {
                localized_copy("Unlock it below to continue").to_string()
            }),
            Some(to_open),
        )
    } else {
        route_tile_body_with_detail(
            wallet_logo(direction.to_kind(), 36.0),
            "To",
            None,
            localized_copy("Another Cube").to_string(),
            String::new(),
            Some(localized_copy("Choose where it goes").to_string()),
            Some(to_open),
        )
    };
    let to = route_tile(TRANSFER_TO_TILE_ID, to_body, to_open, PickerSide::To);
    Row::new()
        .spacing(12)
        .align_y(Alignment::Center)
        .width(Length::Fill)
        .push(
            Container::new(anchored_dropdown(
                from,
                from_open.then(|| from_menu(ctx)),
                Message::Home(HomeMessage::CloseWalletPicker),
            ))
            .width(Length::FillPortion(1)),
        )
        .push(text("→").size(P1_SIZE + 2).style(theme::text::secondary))
        .push(
            Container::new(anchored_dropdown(
                to,
                to_open.then(|| to_menu(ctx)),
                Message::Home(HomeMessage::CloseWalletPicker),
            ))
            .width(Length::FillPortion(1)),
        )
        .into()
}

/// The other Cube's unlock, inside the card: only the method that Cube
/// uses (its PIN boxes, or its passkey), then its checks.
fn unlock_section<'a>(ctx: &TransferCtx<'a>) -> Element<'a, Message> {
    let name = ctx.other.name();
    let wallet = ctx.direction.to_wallet();
    let passkey = ctx.other.choice().is_some_and(|choice| choice.passkey);
    let error = ctx.other.error.map(|error| {
        status_line(
            exclamation_circle_icon(),
            StatusTone::Bad,
            error.to_string(),
        )
    });
    let heading = |label: String| {
        Row::new()
            .spacing(8)
            .align_y(Alignment::Center)
            .push(
                lock_icon()
                    .size(14)
                    .style(|t: &theme::Theme| iced::widget::text::Style {
                        color: Some(t.colors.shell.warn),
                    }),
            )
            .push(text(label).bold())
    };
    match ctx.other.phase {
        UnlockPhase::Credential { busy } if passkey => Column::new()
            .spacing(12)
            .push(heading(localized_format!(
                "Unlock {} with its passkey to transfer into it",
                "Desbloquea {} con su llave de acceso para transferir",
                name
            )))
            .push(dialog_button(
                coincube_ui::locale::copy("Unlock with passkey"),
                Some(key_icon()),
                true,
                (!busy).then_some(Message::Home(HomeMessage::UnlockOtherCubePasskey)),
            ))
            .push(muted(localized_format!(
                "macOS asks you to confirm. {} locks again when this transfer ends.",
                "macOS te pide confirmar. {} se bloquea otra vez al terminar.",
                name
            )))
            .push_maybe(error)
            .into(),
        UnlockPhase::Credential { busy } => {
            let pin_complete = ctx.other.pin.is_some_and(|pin| pin.is_complete());
            Column::new()
                .spacing(12)
                .push(heading(localized_format!(
                    "Enter {}'s PIN to transfer into it",
                    "Introduce la PIN de {} para transferir",
                    name
                )))
                .push(
                    Row::new()
                        .spacing(16)
                        .align_y(Alignment::Center)
                        .push_maybe(ctx.other.pin.map(|pin| {
                            pin.face_view()
                                .map(|message| Message::Home(HomeMessage::OtherCubePin(message)))
                        }))
                        .push(
                            button::primary_loading(
                                None,
                                CopyText::Unlock.get(),
                                busy,
                                (pin_complete && !busy)
                                    .then_some(Message::Home(HomeMessage::UnlockOtherCube)),
                            )
                            .width(Length::Fixed(120.0)),
                        ),
                )
                .push(muted(localized_format!(
                    "Not {}'s PIN. {} locks again when this transfer ends.",
                    "No la PIN de {}. {} se bloquea otra vez al terminar.",
                    ctx.cube_name,
                    name
                )))
                .push_maybe(error)
                .into()
        }
        UnlockPhase::Checks(done) => {
            let state = |row: u8| done.cmp(&row).reverse();
            Column::new()
                .push(check_row(
                    if passkey {
                        "Passkey accepted"
                    } else {
                        "PIN accepted"
                    }
                    .into(),
                    state(0),
                ))
                .push(check_row(
                    if done > 1 {
                        localized_format!(
                            "{name}'s {wallet} checked",
                            "{wallet} de {name} comprobado"
                        )
                    } else {
                        localized_format!(
                            "Checking {name}'s {wallet}…",
                            "Comprobando {wallet} de {name}…"
                        )
                    },
                    state(1),
                ))
                .push(check_row(
                    if done > 2 {
                        "Receive address checked".to_string()
                    } else {
                        "Getting a receive address…".to_string()
                    },
                    state(2),
                ))
                .push_maybe(error.map(|error| Container::new(error).padding([12, 0])))
                .into()
        }
    }
}

/// The fee a direction's quote reported, if there is one yet.
fn quoted_fees(
    direction: TransferDirection,
    prepare_onchain_send_response: Option<&PreparePayOnchainResponse>,
    vault_fees: Option<Amount>,
    spark_send_fee_sat: Option<u64>,
) -> Option<Amount> {
    match direction {
        TransferDirection::LiquidToVault | TransferDirection::LiquidToSpark => {
            prepare_onchain_send_response.map(|response| Amount::from_sat(response.total_fees_sat))
        }
        TransferDirection::VaultToLiquid
        | TransferDirection::VaultToSpark
        | TransferDirection::VaultToOtherSpark
        | TransferDirection::VaultToOtherCube => vault_fees,
        // Spark-sourced: `spark.prepare_send` quoted a `fee_sat` at step
        // 1→2 and the state layer stashed it on `spark_send_fee_sat`.
        TransferDirection::SparkToLiquid
        | TransferDirection::SparkToVault
        | TransferDirection::SparkToOtherSpark
        | TransferDirection::SparkToOtherCube => spark_send_fee_sat.map(Amount::from_sat),
    }
}

/// A fee for display: "None" when the rail charges nothing.
fn fee_text(fee: Amount, unit: BitcoinDisplayUnit, masked: bool) -> String {
    if fee == Amount::ZERO {
        localized_copy("None").to_string()
    } else {
        bitcoin_text(fee, unit, masked)
    }
}

/// Before a quote: what the fee will be. Spark to Spark moves inside Spark
/// and charges nothing; every other route is quoted by Prepare.
fn expected_fee_text(direction: TransferDirection) -> Option<&'static str> {
    (direction == TransferDirection::SparkToOtherSpark).then_some("None · Spark to Spark")
}

/// One label/value row of a card or the rail.
fn kv_row<'a>(
    label: impl Into<String>,
    value: impl Into<String>,
    value_muted: bool,
) -> Element<'a, Message> {
    let value = text(value.into()).size(14);
    Row::new()
        .spacing(16)
        .align_y(Alignment::Center)
        .push(
            text(label.into())
                .size(14)
                .style(theme::text::secondary)
                .width(Length::Fill),
        )
        .push(if value_muted {
            value.style(theme::text::secondary)
        } else {
            value
        })
        .into()
}

/// A section of the Details or Review card.
fn card_section<'a>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    Container::new(content)
        .padding([20, 24])
        .width(Length::Fill)
        .into()
}

/// The Details or Review card: sections divided by hairlines, on the rail's
/// surface.
fn transfer_card<'a>(sections: Vec<Element<'a, Message>>) -> Element<'a, Message> {
    let mut column = Column::new();
    for (index, section) in sections.into_iter().enumerate() {
        if index > 0 {
            column = column.push(iced::widget::rule::horizontal(1));
        }
        column = column.push(section);
    }
    Container::new(column)
        .width(Length::Fill)
        .style(crate::app::view::shared::payment_columns::rail_card_style)
        .into()
}

/// The rail beside the card, as on Send and Swap: what this Cube holds,
/// then this transfer and what it leaves.
fn transfer_rail<'a>(
    ctx: &TransferCtx<'a>,
    amount: Option<Amount>,
    fees: Option<Amount>,
) -> Element<'a, Message> {
    let direction = ctx.direction;
    let mut holdings = Column::new().spacing(12).push(
        text(localized_format!(
            "Available in {}",
            "Disponible en {}",
            ctx.cube_name
        ))
        .size(13)
        .style(theme::text::secondary),
    );
    let mut total = Amount::ZERO;
    for kind in cube_wallets(ctx) {
        total += ctx.balance(kind);
        holdings = holdings.push(
            Row::new()
                .spacing(16)
                .push(text(kind.label()).size(15).width(Length::Fill))
                .push(
                    text(bitcoin_text(
                        ctx.balance(kind),
                        ctx.unit,
                        ctx.balance_masked,
                    ))
                    .size(15),
                ),
        );
    }
    let total_text = match fiat_estimate(total, ctx.fiat_converter) {
        _ if ctx.balance_masked => "••••••".to_string(),
        Some(fiat) => fiat.trim_start_matches("≈ ").to_string(),
        None => bitcoin_text(total, ctx.unit, false),
    };
    holdings = holdings.push(iced::widget::rule::horizontal(1)).push(
        Row::new()
            .spacing(16)
            .align_y(Alignment::Center)
            .push(
                text(localized_copy("Total"))
                    .size(15)
                    .style(theme::text::secondary)
                    .width(Length::Fill),
            )
            .push(text(total_text).size(24).bold()),
    );

    let source = direction.from_kind();
    let spent = amount.map(|amount| amount + fees.unwrap_or(Amount::ZERO));
    let fee_value = match fees {
        Some(fee) => (fee_text(fee, ctx.unit, ctx.balance_masked), false),
        None => match expected_fee_text(direction) {
            Some(fee) => (localized_copy(fee).to_string(), false),
            None => (localized_copy("Quoted at Prepare").to_string(), true),
        },
    };
    let source_after = spent.map_or_else(
        || bitcoin_text(ctx.balance(source), ctx.unit, ctx.balance_masked),
        |spent| {
            bitcoin_text(
                ctx.balance(source)
                    .checked_sub(spent)
                    .unwrap_or(Amount::ZERO),
                ctx.unit,
                ctx.balance_masked,
            )
        },
    );
    let (to_label, to_after, to_muted) = if direction.is_other_cube() {
        (
            localized_format!(
                "{} in {} after",
                "{} en {} después",
                direction.to_wallet(),
                ctx.other.name()
            ),
            match amount {
                Some(amount) if ctx.other.unlocked => {
                    format!("+{}", bitcoin_text(amount, ctx.unit, ctx.balance_masked))
                }
                _ => localized_copy("Locked").to_string(),
            },
            !(ctx.other.unlocked && amount.is_some()),
        )
    } else {
        let to = direction.to_kind();
        (
            localized_format!("{} after", "{} después", to.label()),
            bitcoin_text(
                ctx.balance(to) + amount.unwrap_or(Amount::ZERO),
                ctx.unit,
                ctx.balance_masked,
            ),
            false,
        )
    };
    let this_transfer = Column::new()
        .spacing(10)
        .push(text(localized_copy("This transfer")).size(15).bold())
        .push(kv_row(
            localized_copy("Route"),
            route_text(direction),
            false,
        ))
        .push(match amount {
            Some(amount) => kv_row(
                localized_copy("Amount"),
                bitcoin_text(amount, ctx.unit, ctx.balance_masked),
                false,
            ),
            None => kv_row(localized_copy("Amount"), localized_copy("Not set"), true),
        })
        .push(kv_row(localized_copy("Fee"), fee_value.0, fee_value.1))
        .push(kv_row(
            localized_format!("{} after", "{} después", source.label()),
            source_after,
            false,
        ))
        .push(kv_row(to_label, to_after, to_muted));
    crate::app::view::shared::payment_columns::rail_card(
        Column::new()
            .spacing(20)
            .push(holdings)
            .push(iced::widget::rule::horizontal(1))
            .push(this_transfer),
    )
}

/// Details: one card, as on Send and Swap. From → To tiles open menus of
/// this Cube's wallets and other local Cubes; another Cube unlocks inside
/// the card; then the amount, and Prepare.
fn details_page<'a>(
    ctx: &TransferCtx<'a>,
    page: TransferPage,
    amount: &'a form::Value<String>,
    onchain_send_limit: Option<(u64, u64)>,
    onchain_receive_limit: Option<(u64, u64)>,
) -> TransferScreen<'a> {
    let direction = ctx.direction;
    let source = direction.from_kind();
    let source_empty = ctx.balance(source) == Amount::ZERO;
    let other = direction.is_other_cube();
    let amount_open = page == TransferPage::Amount || !other;
    let parsed = parse_amount(amount, ctx.unit).filter(|_| amount.valid && amount_open);

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
    let limit = |sat| bitcoin_text(Amount::from_sat(sat), ctx.unit, false);
    let limits = match (effective_min, effective_max) {
        (Some(min), Some(max)) => format!("Minimum {} · Maximum {}", limit(min), limit(max)),
        (Some(min), None) => format!("Minimum {}", limit(min)),
        _ => "Loading limits…".to_string(),
    };
    let estimate = match parsed {
        Some(parsed) if !ctx.balance_masked => fiat_estimate(parsed, ctx.fiat_converter),
        _ => None,
    };

    let amount_body: Element<'a, Message> = if amount_open {
        let input = match ctx.unit {
            BitcoinDisplayUnit::BTC => form::Form::new_amount_btc("0", amount, |msg| {
                Message::Home(HomeMessage::AmountEdited(msg))
            }),
            BitcoinDisplayUnit::Sats => form::Form::new_amount_sats("0", amount, |msg| {
                Message::Home(HomeMessage::AmountEdited(msg))
            }),
        }
        .size(18)
        .padding(12);
        Column::new()
            .spacing(8)
            .push(
                Row::new()
                    .spacing(10)
                    .align_y(Alignment::Center)
                    .push(Container::new(input).width(Length::Fill))
                    .push(muted(unit_label(ctx.unit)))
                    .push(
                        button::secondary_compact(None, localized_copy("Max"))
                            .padding([10, 16])
                            .on_press_maybe(
                                (!ctx.max && ctx.balance(source) > Amount::ZERO)
                                    .then_some(Message::Home(HomeMessage::TransferMax)),
                            ),
                    ),
            )
            .push_maybe(ctx.max.then(|| muted(localized_copy(MAX_NOTE))))
            .push(muted(match estimate {
                Some(estimate) => format!("{estimate} · {limits}"),
                None => limits,
            }))
            .into()
    } else {
        muted(if other && page != TransferPage::Wallets {
            localized_format!(
                "Unlock {} to enter an amount.",
                "Desbloquea {} para introducir un importe.",
                ctx.other.name()
            )
        } else {
            localized_copy("Choose where it goes to enter an amount.").to_string()
        })
        .into()
    };

    let fee = match expected_fee_text(direction) {
        Some(fee) => (localized_copy(fee).to_string(), false),
        None => (localized_copy("Shown when you prepare").to_string(), true),
    };
    let summary = Column::new()
        .spacing(10)
        .push(match parsed {
            Some(parsed) => kv_row(
                localized_copy("They receive about"),
                bitcoin_text(parsed, ctx.unit, ctx.balance_masked),
                false,
            ),
            None => kv_row(localized_copy("They receive about"), "—", true),
        })
        .push(kv_row(
            localized_copy("Route"),
            route_text(direction),
            false,
        ))
        .push(kv_row(localized_copy("Fee"), fee.0, fee.1));

    let status = if source_empty && !ctx.balance_masked {
        localized_format!(
            "{} has nothing to transfer.",
            "{} no tiene nada que transferir.",
            source.label()
        )
    } else if other && page == TransferPage::Wallets {
        localized_copy("Choose another Cube in To.").to_string()
    } else if !amount_open {
        localized_format!(
            "Unlock {} to continue.",
            "Desbloquea {} para continuar.",
            ctx.other.name()
        )
    } else if parsed.is_none() {
        localized_copy("Enter an amount.").to_string()
    } else {
        localized_copy("Prepare gets the route and fee. Nothing moves yet.").to_string()
    };
    let footer = Row::new()
        .spacing(16)
        .align_y(Alignment::Center)
        .push(muted(status).width(Length::Fill))
        .push(dialog_button(
            coincube_ui::locale::copy("Prepare"),
            None,
            true,
            parsed.map(|_| Message::Home(HomeMessage::NextStep)),
        ));

    let mut sections = vec![card_section(
        Column::new()
            .spacing(16)
            .push(text(localized_copy("Transfer details")).size(18).bold())
            .push(transfer_route_tiles(ctx, page)),
    )];
    if other && page == TransferPage::Unlock {
        sections.push(card_section(unlock_section(ctx)));
    } else if other && page == TransferPage::OtherCube {
        // The pick didn't start an unlock: no PIN or passkey control could
        // do anything here, so say why and how to try again.
        sections.push(card_section(
            Column::new()
                .spacing(8)
                .push_maybe(ctx.other.error.map(|error| {
                    status_line(
                        exclamation_circle_icon(),
                        StatusTone::Bad,
                        error.to_string(),
                    )
                }))
                .push(muted(localized_format!(
                    "{} can't be unlocked here. Choose it again in To, or pick another wallet.",
                    "{} no se puede desbloquear aquí. Elígelo de nuevo en Para, o elige otra billetera.",
                    ctx.other.name()
                ))),
        ));
    }
    sections.push(card_section(
        Column::new()
            .spacing(10)
            .push(text(localized_copy("Amount")).bold())
            .push(amount_body),
    ));
    sections.push(card_section(summary));
    sections.push(card_section(footer));

    transfer_dialog(
        ctx,
        0,
        None,
        Some(Message::Home(HomeMessage::LeaveTransfer)),
        crate::app::view::shared::payment_columns::payment_columns(
            transfer_card(sections),
            transfer_rail(ctx, parsed, None),
        ),
    )
}

/// The route a direction takes, for Review.
fn route_text(direction: TransferDirection) -> &'static str {
    match direction {
        TransferDirection::LiquidToVault | TransferDirection::LiquidToSpark => {
            "Liquid → Bitcoin swap, then on-chain"
        }
        TransferDirection::VaultToLiquid | TransferDirection::SparkToLiquid => {
            "Bitcoin on-chain, then swap to Liquid"
        }
        TransferDirection::VaultToSpark
        | TransferDirection::SparkToVault
        | TransferDirection::VaultToOtherCube
        | TransferDirection::SparkToOtherCube
        | TransferDirection::VaultToOtherSpark => "Bitcoin on-chain",
        TransferDirection::SparkToOtherSpark => "Spark → Spark",
    }
}

/// One row of P-Transfer's `.dl`: a 180px muted label, then the value.
fn dl_row<'a>(
    label: &'static str,
    value: impl Into<Element<'a, Message>>,
    total: bool,
) -> Element<'a, Message> {
    Column::new()
        .push(
            Row::new()
                .spacing(16)
                .padding([12, 0])
                .push(
                    Container::new(if total {
                        text(label).bold()
                    } else {
                        text(label).style(theme::text::secondary)
                    })
                    .width(Length::Fixed(180.0)),
                )
                .push(Container::new(value).width(Length::Fill)),
        )
        .push(iced::widget::rule::horizontal(1))
        .into()
}

fn description_list<'a>(rows: Vec<Element<'a, Message>>) -> Element<'a, Message> {
    let mut list = Column::new().push(iced::widget::rule::horizontal(1));
    for row in rows {
        list = list.push(row);
    }
    list.into()
}

/// The full destination address with a copy control. For a wallet of this
/// Cube its label and QR code sit behind a disclosure.
fn receiving_address<'a>(
    cube_id: &str,
    addr: &'a coincube_core::miniscript::bitcoin::Address,
    labels: &'a std::collections::HashMap<String, String>,
    labels_editing: &'a std::collections::HashMap<String, form::Value<String>>,
    expanded: bool,
    details: bool,
) -> Element<'a, Message> {
    let addr_str = addr.to_string();
    let address = Container::new(
        Row::new()
            .spacing(12)
            .align_y(Alignment::Center)
            .push(
                coincube_ui::component::tenshu::mono(addr_str.clone())
                    .size(P1_SIZE)
                    .wrapping(iced::widget::text::Wrapping::WordOrGlyph)
                    .width(Length::Fill),
            )
            .push(
                Button::new(clipboard_icon().style(theme::text::secondary))
                    .padding(6)
                    .style(theme::button::transparent_border)
                    .on_press(Message::CopyWithToast(
                        addr_str.clone(),
                        "Address copied".to_string(),
                    )),
            ),
    )
    .padding([12, 16])
    .width(Length::Fill)
    .style(review_panel_style);

    let disclosure = details.then(|| {
        let editor = Column::new()
            .spacing(10)
            .push(if let Some(label) = labels_editing.get(&addr_str) {
                crate::app::view::vault::label::label_editing(
                    vec![addr_str.clone()],
                    label,
                    P1_SIZE,
                )
            } else {
                crate::app::view::vault::label::label_editable(
                    vec![addr_str.clone()],
                    labels.get(&addr_str),
                    P1_SIZE,
                )
            })
            .push(
                button::secondary(None, localized_copy("Show QR code"))
                    .on_press(Message::ShowQrCode(0)),
            );
        coincube_ui::component::tenshu::disclosure(
            landing_control_id(cube_id, "transfer", "address-details"),
            "Address details",
            expanded,
            Message::FocusAction,
        )
        .caption(localized_copy("Label, QR code"))
        .on_toggle(|_| Message::SelectAddress(addr.clone()))
        .content(editor)
    });

    Column::new()
        .spacing(10)
        .push(text(localized_copy("Receiving address")).bold())
        .push(address)
        .push_maybe(disclosure)
        .into()
}

/// Review: P-Transfer's summary list, the destination address, the Vault fee
/// rate where the source is the Vault, then the confirm action.
#[allow(clippy::too_many_arguments)]
fn review_page<'a>(
    ctx: &TransferCtx<'a>,
    amount: &'a form::Value<String>,
    receive_address: Option<&'a coincube_core::miniscript::bitcoin::Address>,
    destination_ready: bool,
    transfer_action_ready: bool,
    labels: &'a std::collections::HashMap<String, String>,
    labels_editing: &'a std::collections::HashMap<String, form::Value<String>>,
    address_expanded: bool,
    is_sending: bool,
    is_tx_signed: bool,
    prepare_onchain_send_response: Option<&'a PreparePayOnchainResponse>,
    vault_fees: Option<Amount>,
    transfer_feerate: &'a form::Value<String>,
    transfer_feerate_loading: Option<crate::app::view::shared::feerate_picker::FeeratePreset>,
    spark_send_fee_sat: Option<u64>,
) -> TransferScreen<'a> {
    let direction = ctx.direction;
    let unit = ctx.unit;
    let parsed = parse_amount(amount, unit);
    let fees = quoted_fees(
        direction,
        prepare_onchain_send_response,
        vault_fees,
        spark_send_fee_sat,
    );
    let with_fiat = |value: Amount| -> String {
        let bitcoin = bitcoin_text(value, unit, ctx.balance_masked);
        match fiat_estimate(value, ctx.fiat_converter).filter(|_| !ctx.balance_masked) {
            Some(fiat) => format!("{bitcoin} · {fiat}"),
            None => bitcoin,
        }
    };
    let receiving = if direction.is_other_cube() {
        format!(
            "{} · {} · {}",
            direction.to_wallet(),
            ctx.other.name(),
            network_name(ctx.chain)
        )
    } else {
        format!("{} · {}", direction.to_wallet(), ctx.cube_name)
    };
    let mut rows = vec![
        dl_row(
            "Sending from",
            text(format!("{} · {}", direction.from_wallet(), ctx.cube_name)),
            false,
        ),
        dl_row(localized_copy("Receiving wallet"), text(receiving), false),
        dl_row("Route", text(route_text(direction)), false),
        dl_row(
            CopyText::BitcoinNetwork.get(),
            text(crate::chain::canonical_network_name(ctx.chain)),
            false,
        ),
    ];
    if let Some(parsed) = parsed {
        rows.push(dl_row(
            localized_copy("Amount"),
            text(with_fiat(parsed)),
            false,
        ));
    }
    rows.push(dl_row(
        localized_copy("Fees"),
        match fees {
            Some(fees) => text(fee_text(fees, unit, ctx.balance_masked)),
            None => muted(localized_copy("Calculating…")),
        },
        false,
    ));
    if let (Some(fees), Some(parsed)) = (fees, parsed) {
        rows.push(dl_row(
            "Total",
            text(bitcoin_text(fees + parsed, unit, ctx.balance_masked)).bold(),
            true,
        ));
    }

    let destination: Element<'a, Message> = if let Some(address) = ctx
        .other
        .spark_address
        .filter(|_| direction == TransferDirection::SparkToOtherSpark && destination_ready)
    {
        group(Some(localized_copy("Spark address").into()), text(address))
    } else {
        match receive_address {
            Some(addr) => receiving_address(
                ctx.cube_id,
                addr,
                labels,
                labels_editing,
                address_expanded,
                !direction.is_other_cube(),
            ),
            None => muted(localized_copy("Getting the receiving address…")).into(),
        }
    };

    // §Design section 9: Vault-sourced transfers build a standard on-chain
    // Bitcoin tx, so the user controls the sats/vbyte rate. Presets fetch a
    // mempool estimate and write it into the text input.
    let feerate = (direction.from_kind() == WalletKind::Vault).then(|| {
        Column::new()
            .spacing(10)
            .push(text(localized_copy("Fee rate (sats/vbyte)")).bold())
            .push(
                crate::app::view::shared::feerate_picker::feerate_presets_row::<Message>(
                    transfer_feerate_loading,
                    |preset| Message::Home(HomeMessage::FetchTransferFeeratePreset(preset)),
                    is_tx_signed || is_sending,
                ),
            )
            .push(crate::app::view::shared::feerate_picker::feerate_input::<
                Message,
                _,
            >(
                transfer_feerate,
                |s| Message::Home(HomeMessage::SetTransferFeerate(s)),
                is_tx_signed || is_sending,
            ))
    });

    let primary = match direction {
        // Vault-sourced: sign the on-chain tx first, then broadcast. Shared
        // signer path handles Liquid, Spark and another Cube's Vault.
        TransferDirection::VaultToLiquid
        | TransferDirection::VaultToSpark
        | TransferDirection::VaultToOtherSpark
        | TransferDirection::VaultToOtherCube => {
            if is_tx_signed {
                button::primary_loading(
                    None,
                    coincube_ui::locale::copy("Confirm transfer"),
                    is_sending,
                    transfer_action_ready.then_some(Message::Home(HomeMessage::ConfirmTransfer)),
                )
            } else {
                button::primary_loading(
                    None,
                    coincube_ui::locale::copy("Sign transaction"),
                    is_sending,
                    (transfer_feerate.valid && destination_ready)
                        .then_some(Message::Home(HomeMessage::SignVaultToLiquidTx)),
                )
            }
        }
        TransferDirection::LiquidToVault | TransferDirection::LiquidToSpark => {
            button::primary_loading(
                None,
                coincube_ui::locale::copy("Confirm transfer"),
                is_sending,
                transfer_action_ready.then_some(Message::Home(HomeMessage::ConfirmTransfer)),
            )
        }
        // Spark-sourced: the prepare handle was fetched at step 1→2. Confirm
        // calls `spark.send_payment(handle)` synchronously from the UI's
        // perspective; stage flips to PendingDeposit on success.
        TransferDirection::SparkToLiquid
        | TransferDirection::SparkToVault
        | TransferDirection::SparkToOtherSpark
        | TransferDirection::SparkToOtherCube => button::primary_loading(
            None,
            coincube_ui::locale::copy("Confirm transfer"),
            is_sending,
            transfer_action_ready.then_some(Message::Home(HomeMessage::ConfirmSparkSend)),
        ),
    };

    // You transfer → They receive, as Swap's Review opens.
    let side = |caption: &'static str, value: Option<Amount>, place: String| {
        Column::new()
            .spacing(4)
            .width(Length::Fill)
            .push(
                text(localized_copy(caption))
                    .size(13)
                    .style(theme::text::secondary),
            )
            .push(
                text(value.map_or_else(
                    || "—".to_string(),
                    |value| bitcoin_text(value, unit, ctx.balance_masked),
                ))
                .size(22)
                .bold(),
            )
            .push_maybe(
                value
                    .and_then(|value| fiat_estimate(value, ctx.fiat_converter))
                    .filter(|_| !ctx.balance_masked)
                    .map(muted),
            )
            .push(muted(place))
    };
    let summary = Row::new()
        .spacing(16)
        .align_y(Alignment::Center)
        .push(side(
            "You transfer",
            parsed.map(|parsed| parsed + fees.unwrap_or(Amount::ZERO)),
            localized_format!(
                "from {} in {}",
                "desde {} en {}",
                direction.from_wallet(),
                ctx.cube_name
            ),
        ))
        .push(arrow_right().size(16).style(theme::text::secondary))
        .push(side(
            "They receive",
            parsed,
            localized_format!(
                "in {} · {}",
                "en {} · {}",
                direction.to_wallet(),
                if direction.is_other_cube() {
                    ctx.other.name()
                } else {
                    ctx.cube_name
                }
            ),
        ));
    let rail = transfer_rail(ctx, parsed, fees);
    transfer_dialog(
        ctx,
        ctx.review_index(),
        other_cube_context(ctx, true, false),
        (!is_sending).then_some(Message::Home(HomeMessage::LeaveTransfer)),
        crate::app::view::shared::payment_columns::payment_columns(
            legacy_form(
                Column::new()
                    .spacing(20)
                    .push(summary)
                    .push(description_list(rows))
                    .push(destination)
                    .push_maybe(feerate)
                    .push_maybe(direction.is_other_cube().then(|| {
                        status_line(
                            lock_icon(),
                            StatusTone::Muted,
                            localized_format!(
                                "{} locks again once sent, or if you leave.",
                                "{} cierra otra vez una vez enviado, o si te vas.",
                                ctx.other.name()
                            ),
                        )
                    }))
                    .push(page_actions(
                        Some(back_button(
                            "Edit",
                            (!is_sending).then_some(Message::Home(HomeMessage::PreviousStep)),
                        )),
                        primary.width(Length::Fixed(200.0)).into(),
                    )),
            ),
            rail,
        ),
    )
}

/// Done: the sender has submitted the payment. On-chain deposits can still
/// be awaiting confirmations or a Spark claim.
fn done_page<'a>(ctx: &TransferCtx<'a>, amount: &'a form::Value<String>) -> TransferScreen<'a> {
    let direction = ctx.direction;
    let to = if direction.is_other_cube() {
        format!("{} · {}", direction.to_wallet(), ctx.other.name())
    } else {
        format!("{} · {}", direction.to_wallet(), ctx.cube_name)
    };
    let status = if direction == TransferDirection::SparkToOtherSpark {
        status_line(check_circle_icon(), StatusTone::Good, "Sent".into())
    } else {
        status_line(clock_icon(), StatusTone::Warn, "Pending".into())
    };
    let mut rows = vec![dl_row(
        "Status",
        Row::new()
            .spacing(4)
            .align_y(Alignment::Center)
            .push(status)
            .push(muted("· you can leave this page")),
        false,
    )];
    if let Some(parsed) = parse_amount(amount, ctx.unit) {
        rows.push(dl_row(
            localized_copy("Amount"),
            text(bitcoin_text(parsed, ctx.unit, ctx.balance_masked)),
            false,
        ));
    }
    rows.push(dl_row(localized_copy("To"), text(to), false));
    transfer_dialog(
        ctx,
        ctx.steps().len(),
        other_cube_context(ctx, false, true),
        Some(Message::Home(HomeMessage::LeaveTransfer)),
        legacy_form(
            Column::new()
                .spacing(20)
                .push(
                    Row::new()
                        .spacing(10)
                        .align_y(Alignment::Center)
                        .push(check_circle_icon().size(28).style(|t: &theme::Theme| {
                            iced::widget::text::Style {
                                color: Some(t.colors.shell.good),
                            }
                        }))
                        .push(text(localized_copy("Transfer sent")).size(18).bold()),
                )
                .push(description_list(rows))
                .push_maybe(
                    (direction == TransferDirection::VaultToOtherSpark).then(|| {
                        muted(localized_format!(
                    "Spark credits the deposit after it is claimed. Open {} to check its status.",
                    "Spark acredita el depósito después de reclamarlo. Abre {} para ver su estado.",
                    ctx.other.name()
                ))
                    }),
                )
                .push(
                    Row::new()
                        .spacing(10)
                        .align_y(Alignment::Center)
                        .push(dialog_button(
                            coincube_ui::locale::copy("See it in Activity"),
                            None,
                            true,
                            Some(Message::Home(HomeMessage::OpenTransferActivity)),
                        ))
                        .push(dialog_button(
                            coincube_ui::locale::copy(match ctx.origin {
                                Some(WalletKind::Spark) => "Back to Spark",
                                Some(WalletKind::Vault) => "Back to Vault",
                                Some(WalletKind::Liquid) => "Back to Liquid",
                                None => "Back to Overview",
                            }),
                            None,
                            false,
                            Some(Message::Home(HomeMessage::LeaveTransfer)),
                        ))
                        .push(Space::new().width(Length::Fill))
                        .push(dialog_button(
                            coincube_ui::locale::copy("New transfer"),
                            None,
                            false,
                            Some(Message::Home(HomeMessage::RestartTransfer)),
                        )),
                ),
        ),
    )
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

/// Why a wallet-to-wallet transfer couldn't go through.
///
/// The transfer flow spans three wallets and six directions, and its ~20
/// failure points used to funnel into one `String`. That left the screen with
/// two bad options: echo the underlying SDK trace, or — once that was
/// sanitised — say "Transfer failed" to everyone, whether their Vault simply
/// wasn't open or a broadcast had just been refused.
///
/// The distinction that matters most to someone moving money is **whether
/// anything was sent**. Every variant sits on one side of that line and
/// [`TransferError::failure_point`] is what says which, so the copy can promise "your
/// funds haven't moved" only where that is actually true.
///
/// Each variant carries the technical `detail` for the log. None of it is
/// rendered — `crate::user_error` turns the variant into copy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransferError {
    /// A wallet this transfer needs isn't open or configured in this Cube.
    /// Reached by reconciling state the dashboard thought it had, so it is a
    /// dead end rather than something to retry.
    WalletUnavailable(WalletKind),
    /// The receiving wallet couldn't issue a deposit address.
    NoDepositAddress { to: WalletKind, detail: String },
    /// Working out the network fee, or fetching the swap limits that bound it,
    /// failed. Nothing has been prepared, let alone sent.
    FeeEstimateFailed { detail: String },
    /// The fee-rate field doesn't hold a usable value. Reachable only by
    /// bypassing the form's own validation.
    InvalidFeerate,
    /// The prepared send is gone: Spark's prepare handle is single-use, and a
    /// failed attempt consumes it. The user has to go back and re-enter the
    /// amount, which is a different instruction from "try again".
    PreparationExpired,
    /// The address the prepare step returned isn't valid for the active
    /// network. An internal invariant violation, not anything the user did or
    /// can fix — so the copy points at support rather than at a retry.
    DestinationRejected { detail: String },
    /// The send was submitted and refused. The `from` wallet is the one whose
    /// funds were being moved.
    BroadcastFailed { from: WalletKind, detail: String },
    /// The sending wallet did not confirm the outcome after submission.
    BroadcastOutcomeUnknown { detail: String },
}

/// How far a transfer got before it failed — the only thing a person moving
/// money reliably wants to know first.
///
/// Distinct from [`TransferStage`], which tracks a *successful* transfer's
/// progress towards settlement. This one only ever describes a failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransferFailurePoint {
    /// Nothing was submitted. The user's balances are untouched, and saying so
    /// is the most useful sentence on the screen.
    BeforeSending,
    /// A send was submitted to a wallet or swap service and refused. Funds
    /// were not moved, but we stop short of the same flat reassurance: the
    /// request did reach a backend.
    WhileSending,
    /// The transfer may have been submitted and its outcome is unknown.
    OutcomeUnknown,
}

impl TransferError {
    pub fn failure_point(&self) -> TransferFailurePoint {
        match self {
            Self::BroadcastFailed { .. } => TransferFailurePoint::WhileSending,
            Self::BroadcastOutcomeUnknown { .. } => TransferFailurePoint::OutcomeUnknown,
            Self::WalletUnavailable(_)
            | Self::NoDepositAddress { .. }
            | Self::FeeEstimateFailed { .. }
            | Self::InvalidFeerate
            | Self::PreparationExpired
            | Self::DestinationRejected { .. } => TransferFailurePoint::BeforeSending,
        }
    }

    /// The technical cause, for the log only.
    pub fn detail(&self) -> String {
        match self {
            Self::WalletUnavailable(w) => format!("{} wallet unavailable", w.label()),
            Self::NoDepositAddress { to, detail } => {
                format!("no deposit address from {}: {}", to.label(), detail)
            }
            Self::FeeEstimateFailed { detail } => format!("fee estimate: {detail}"),
            Self::InvalidFeerate => "feerate field did not parse".to_string(),
            Self::PreparationExpired => "prepared send handle missing or consumed".to_string(),
            Self::DestinationRejected { detail } => {
                format!("prepared destination rejected: {detail}")
            }
            Self::BroadcastFailed { from, detail } => {
                format!("broadcast from {}: {}", from.label(), detail)
            }
            Self::BroadcastOutcomeUnknown { detail } => {
                format!("broadcast outcome unknown: {detail}")
            }
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
/// Exhaustive over the legal directed pairs and cross-Cube routes. Using an explicit
/// enum over a `struct { from: WalletKind, to: WalletKind }` gives compiler-driven
/// exhaustiveness in match arms and keeps same-Cube and cross-Cube routes distinct.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransferDirection {
    LiquidToVault,
    VaultToLiquid,
    LiquidToSpark,
    SparkToLiquid,
    VaultToSpark,
    SparkToVault,
    /// This Cube's Vault to the Vault of another local Bitcoin mainnet Cube,
    /// unlocked in place for this transfer only. Never built from a wallet
    /// pair: the To list's "Another Cube…" row selects it.
    VaultToOtherCube,
    /// This Cube's Spark wallet to another local Bitcoin mainnet Cube's Vault.
    SparkToOtherCube,
    VaultToOtherSpark,
    SparkToOtherSpark,
}

impl TransferDirection {
    pub fn pair(&self) -> (WalletKind, WalletKind) {
        match self {
            Self::LiquidToVault => (WalletKind::Liquid, WalletKind::Vault),
            Self::VaultToLiquid => (WalletKind::Vault, WalletKind::Liquid),
            Self::LiquidToSpark => (WalletKind::Liquid, WalletKind::Spark),
            Self::SparkToLiquid => (WalletKind::Spark, WalletKind::Liquid),
            Self::VaultToSpark | Self::VaultToOtherSpark => (WalletKind::Vault, WalletKind::Spark),
            Self::SparkToVault | Self::SparkToOtherCube => (WalletKind::Spark, WalletKind::Vault),
            Self::VaultToOtherCube => (WalletKind::Vault, WalletKind::Vault),
            Self::SparkToOtherSpark => (WalletKind::Spark, WalletKind::Spark),
        }
    }

    pub fn is_other_cube(&self) -> bool {
        matches!(
            self,
            Self::VaultToOtherCube
                | Self::SparkToOtherCube
                | Self::VaultToOtherSpark
                | Self::SparkToOtherSpark
        )
    }

    pub fn to_other_cube(from: WalletKind, chain: ChainId) -> Option<Self> {
        Self::to_other_wallet(from, WalletKind::Vault, chain)
    }

    pub fn to_other_wallet(from: WalletKind, to: WalletKind, chain: ChainId) -> Option<Self> {
        match (from, to, chain) {
            (WalletKind::Vault, WalletKind::Vault, ChainId::Bitcoin) => {
                Some(Self::VaultToOtherCube)
            }
            (WalletKind::Spark, WalletKind::Vault, ChainId::Bitcoin) => {
                Some(Self::SparkToOtherCube)
            }
            (WalletKind::Vault, WalletKind::Spark, ChainId::Bitcoin) => {
                Some(Self::VaultToOtherSpark)
            }
            (WalletKind::Spark, WalletKind::Spark, ChainId::Bitcoin) => {
                Some(Self::SparkToOtherSpark)
            }
            _ => None,
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
        if self.is_other_cube() {
            return format!("{} → another Cube", self.from_wallet());
        }
        format!("{} → {}", self.from_wallet(), self.to_wallet())
    }
}

pub struct GlobalViewConfig<'a> {
    pub cube_id: &'a str,
    pub liquid_balance: Amount,
    /// Spark wallet BTC balance in sats. Defaults to `ZERO` while
    /// the first `get_info` round-trip is in flight or if the
    /// bridge subprocess is currently down.
    pub spark_balance: Amount,
    pub spark_btc_balance: Amount,
    pub spark_usdb_balance: Option<&'a coincube_spark_protocol::UsdbBalanceSnapshot>,
    pub spark_balance_error: bool,
    pub usdt_balance: u64,
    pub usdt_balance_error: bool,
    pub pending_liquid_send_sats: u64,
    pub pending_usdt_send_sats: u64,
    pub pending_liquid_receive_sats: u64,
    pub pending_usdt_receive_sats: u64,
    pub vault_pending_send_sats: u64,
    pub vault_pending_receive_sats: u64,
    pub vault_balance: Amount,
    pub imported_balance: Option<Amount>,
    pub imported_installed: bool,
    pub fiat_converter: Option<FiatAmountConverter>,
    pub balance_masked: bool,
    /// Session-local Overview section choices; global Hide overrides both.
    pub overview_total_hidden: bool,
    pub overview_wallets_hidden: bool,
    /// True until every applicable wallet has reported its first balance
    /// (or its first error). Drives the Total Balance loading placeholder
    /// so the aggregate doesn't briefly read as a misleadingly low partial.
    pub total_balance_loading: bool,
    /// True once the bridge has corroborated the Spark balance via a
    /// `Synced` event. While false the Spark card renders a `…`
    /// placeholder — avoids flashing a stale pre-sync value (e.g. zero
    /// before the session's incoming payments have landed).
    pub spark_balance_loaded: bool,
    /// True once the Liquid bridge has reported its first L-BTC
    /// balance. Drives the L-BTC row's animated-dots placeholder.
    pub liquid_balance_loaded: bool,
    /// A balance fetch failed, so the row shows "Balance unavailable" rather
    /// than the zero it never managed to overwrite.
    pub liquid_balance_error: bool,
    /// True once the Liquid bridge has reported USDt balance (or an
    /// explicit error, captured in `usdt_balance_error`). Drives the
    /// USDt row's animated-dots placeholder.
    pub usdt_balance_loaded: bool,
    /// True once the Vault daemon has produced its first cache poll
    /// (`blockheight > 0`). Drives the Vault card's animated-dots
    /// placeholder and suppresses pending indicators until the
    /// underlying balance is authoritative.
    pub vault_loaded: bool,
    /// Confirmed, unspent coins can be spent through a recovery path.
    pub recovery_available: bool,
    /// "Last known Vault balance · not updating since {time}" while the
    /// Vault's recent polls fail.
    pub vault_stale: Option<String>,
    /// Global fiat-native vs. bitcoin-native display preference. Mirrored
    /// from `cache.display_mode`.
    pub display_mode: crate::app::settings::display::DisplayMode,
    pub has_vault: bool,
    pub has_imported_bitcoin: bool,
    /// Whether this cube has a working Spark backend. Mirrors
    /// `spark_backend.is_some()` in the state layer. Drives the Spark card
    /// visibility and feeds [`transfer_available`].
    pub has_spark: bool,
    /// Whether the Liquid wallet is shown at all — the sunset gate AND a
    /// Liquid backend on the current network
    /// ([`features::liquid_wallet_usable`](crate::app::features::liquid_wallet_usable)).
    /// Drives the Liquid card, the Liquid row in the wallet picker, and
    /// Liquid's contribution to the Total Balance. Liquid used to be
    /// unconditional on every cube; it no longer is.
    pub has_liquid: bool,
    pub current_view: HomeView,
    pub transfer_direction: Option<TransferDirection>,
    pub transfer_from: Option<WalletKind>,
    pub transfer_to: Option<WalletKind>,
    /// When `Some`, the wallet-picker popup is open and editing the named side.
    pub wallet_picker: Option<PickerSide>,
    pub entered_amount: &'a form::Value<String>,
    pub receive_address: Option<&'a coincube_core::miniscript::bitcoin::Address>,
    /// True only when the destination was returned for the current transfer request.
    pub destination_ready: bool,
    /// Current destination plus the direction-specific prepared response/handle.
    pub transfer_action_ready: bool,
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
    /// This Cube's name and chain, for Overview and the transfer dialog.
    pub cube_name: &'a str,
    pub vault_alias: Option<&'a str>,
    pub chain: ChainId,
    pub light_theme: bool,
    /// Which page step 1 of the transfer shows.
    pub transfer_page: TransferPage,
    /// The unit the transfer amount is entered and shown in.
    pub transfer_unit: BitcoinDisplayUnit,
    /// Max is on: the transfer sends the source's whole balance, fees included.
    pub transfer_max: bool,
    pub other_cube: OtherCubeView<'a>,
    /// The wallet page this transfer was opened from, which leaving returns
    /// to; `None` for the Overview.
    pub transfer_origin: Option<WalletKind>,
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

/// The Overview, and the Transfer page that replaces it while a transfer
/// step (1–3) is active.
pub fn global_home_view<'a>(
    config: GlobalViewConfig<'a>,
) -> (Element<'a, Message>, Option<TransferScreen<'a>>) {
    let GlobalViewConfig {
        cube_id,
        liquid_balance,
        spark_balance,
        spark_btc_balance,
        spark_usdb_balance,
        spark_balance_error,
        usdt_balance,
        usdt_balance_error,
        pending_liquid_send_sats,
        pending_usdt_send_sats,
        pending_liquid_receive_sats,
        pending_usdt_receive_sats,
        vault_pending_send_sats,
        vault_pending_receive_sats,
        vault_balance,
        imported_balance,
        imported_installed,
        fiat_converter,
        balance_masked,
        overview_total_hidden,
        overview_wallets_hidden,
        total_balance_loading,
        spark_balance_loaded,
        liquid_balance_loaded,
        liquid_balance_error,
        usdt_balance_loaded,
        vault_loaded,
        recovery_available,
        vault_stale,
        display_mode,
        has_vault,
        has_imported_bitcoin,
        has_spark,
        has_liquid,
        current_view,
        transfer_direction,
        transfer_from: _,
        transfer_to: _,
        wallet_picker,
        entered_amount,
        receive_address,
        destination_ready,
        transfer_action_ready,
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
        cube_name,
        vault_alias,
        chain,
        light_theme: _,
        transfer_page,
        transfer_unit,
        transfer_max,
        other_cube,
        transfer_origin,
    } = config;

    let overview_masks = OverviewMasks::new(
        balance_masked,
        overview_total_hidden,
        overview_wallets_hidden,
    );
    let wallet_balance_masked = overview_masks.wallets;
    let total_balance_masked = overview_masks.total;

    // Step machine:
    //   0 — overview (wallet cards; Transfer is the page head's action)
    //   1 — Wallets, then Amount (and, to another Cube, Other Cube and
    //       Unlock in between): see `TransferPage`
    //   2 — Review
    //   3 — Done
    // Steps 1–3 are a dialog the state layer lays over the overview.
    let _ = pending_vault_incoming;
    let transfer = transfer_direction.and_then(|direction| {
        let ctx = TransferCtx {
            cube_id,
            cube_name,
            chain,
            direction,
            has_vault,
            has_spark,
            has_liquid,
            liquid_balance,
            spark_balance,
            vault_balance,
            fiat_converter,
            unit: transfer_unit,
            max: transfer_max,
            balance_masked,
            other: other_cube,
            menu: wallet_picker,
            origin: transfer_origin,
        };
        match current_view.step {
            1 => Some(details_page(
                &ctx,
                transfer_page,
                entered_amount,
                onchain_send_limit,
                onchain_receive_limit,
            )),
            2 => Some(review_page(
                &ctx,
                entered_amount,
                receive_address,
                destination_ready,
                transfer_action_ready,
                labels,
                labels_editing,
                address_expanded,
                is_sending,
                is_tx_signed,
                prepare_onchain_send_response,
                spend_tx_fees,
                transfer_feerate,
                transfer_feerate_loading,
                spark_send_fee_sat,
            )),
            3 => Some(done_page(&ctx, entered_amount)),
            _ => None,
        }
    });

    // --- Combined Liquid card (L-BTC + USDt) ---
    // USDt is pegged to USD, so always use BTC/USD price for conversion.
    // When the user's fiat currency is USD, btc_usd_price == converter.price_per_btc().
    // When it differs (e.g. EUR), we must use the dedicated USD price to avoid mispricing.
    let usable_usd_quote = btc_usd_price.filter(|price| *price > 0.0 && price.is_finite());
    let usdt_fiat_value = usdt_balance as f64 / 1e8; // USDt base units → dollars
    let usdt_as_sats = usable_usd_quote
        .map(|btc_price_usd| (usdt_fiat_value / btc_price_usd * 1e8) as u64)
        .unwrap_or(0);
    // A gated-off Liquid wallet contributes nothing to the aggregate. Its
    // balances are zero in that state anyway (the state layer skips the
    // loaders), but stating it here keeps the Total Balance honest even if a
    // stale value ever survived the gate flipping.
    let total_sats = if has_liquid {
        liquid_balance.to_sat().saturating_add(usdt_as_sats)
    } else {
        0
    };
    let total_amount = Amount::from_sat(total_sats);
    let total_fiat = fiat_converter.as_ref().map(|c| c.convert(total_amount));
    let lbtc_fiat = fiat_converter.as_ref().map(|c| c.convert(liquid_balance));

    // Global Total Balance: aggregate across Spark + Liquid (L-BTC + USDt-as-sats) + Vault.
    // Callers pass `has_spark`/`has_vault` to gate visibility, but zero-balance
    // default values for absent wallets don't skew the total.
    // An imported Vault can report coins before its bounded history search is
    // complete. Those coins are not an authoritative balance yet.
    let imported_balance_loaded =
        has_imported_bitcoin && imported_installed && imported_balance.is_some();
    let display_vault_balance = if has_imported_bitcoin && !imported_balance_loaded {
        Amount::ZERO
    } else {
        vault_balance
    };
    // A bridge response received before Synced can contain persisted values
    // from the prior session. Neither BTC nor USDB may enter the aggregate or
    // read as a holding until the existing corroboration gate opens.
    let trusted_spark_usdb_balance = spark_usdb_balance.filter(|_| spark_balance_loaded);
    let spark_usdb_held = trusted_spark_usdb_balance.is_some_and(|holding| holding.balance > 0);
    let spark_usdb_as_sats = trusted_spark_usdb_balance
        .map(|holding| {
            crate::app::breez_spark::assets::stable_token_as_sats_u128(
                holding.balance,
                holding.decimals,
                usable_usd_quote,
            )
        })
        .unwrap_or(0);
    let spark_display_balance = Amount::from_sat(if has_spark && spark_balance_loaded {
        spark_btc_balance
            .to_sat()
            .saturating_add(spark_usdb_as_sats)
    } else {
        0
    });
    let spark_valuation_partial = spark_usdb_held && usable_usd_quote.is_none();
    let global_total_sats = spark_display_balance
        .to_sat()
        .saturating_add(total_sats)
        .saturating_add(display_vault_balance.to_sat());
    let global_total_amount = Amount::from_sat(global_total_sats);
    let global_total_fiat = fiat_converter
        .as_ref()
        .map(|c| c.convert(global_total_amount));
    let total_partial = (has_spark && (spark_balance_error || spark_valuation_partial))
        || (has_liquid
            && (liquid_balance_error
                || usdt_balance_error
                || (usdt_balance > 0 && usable_usd_quote.is_none())));

    let orange_outline_btn =
        |wallet: &'static str, control: &'static str, msg: Message| -> Element<'a, Message> {
            landing_button(
                landing_control_id(cube_id, wallet, control),
                localized_copy("Receive"),
                None,
                Message::FocusAction,
                Some(msg),
                theme::button::orange_outline,
            )
            .width(Length::Fixed(90.0))
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
                    "lbtc", "liquid", 32.0,
                ))
                .push(
                    text("L-BTC")
                        .size(P1_SIZE)
                        .bold()
                        .width(Length::Fixed(60.0)),
                )
                .push(if wallet_balance_masked {
                    Row::new().push(text("********").size(P1_SIZE))
                } else if liquid_balance_error {
                    Row::new().push(
                        text(localized_copy("Balance unavailable"))
                            .size(P1_SIZE)
                            .color(color::RED),
                    )
                } else if !liquid_balance_loaded {
                    Row::new().push(spinner::typing_text_carousel(
                        "...",
                        false,
                        Duration::from_millis(400),
                        |content| text(content).size(P1_SIZE).style(theme::text::secondary),
                    ))
                } else {
                    amount_with_size_and_unit(&liquid_balance, P1_SIZE, bitcoin_unit)
                })
                .push_maybe(
                    (!wallet_balance_masked && liquid_balance_loaded && !liquid_balance_error)
                        .then(|| {
                            lbtc_fiat
                                .map(|f| f.to_text().size(P2_SIZE).style(theme::text::secondary))
                        })
                        .flatten(),
                )
                .push(Space::new().width(Length::Fill))
                .push(
                    landing_button(
                        landing_control_id(cube_id, "Liquid", "send-lbtc"),
                        localized_copy("Send"),
                        None,
                        Message::FocusAction,
                        Some(Message::Home(HomeMessage::SendAsset(
                            crate::app::state::liquid::send::SendAsset::Lbtc,
                        ))),
                        theme::button::primary,
                    )
                    .width(Length::Fixed(90.0)),
                )
                .push(orange_outline_btn(
                    "Liquid",
                    "receive-lbtc",
                    Message::Home(HomeMessage::ReceiveAsset(
                        crate::app::state::liquid::send::SendAsset::Lbtc,
                    )),
                )),
        )
        .push_maybe(
            (!wallet_balance_masked
                && liquid_balance_loaded
                && !liquid_balance_error
                && pending_liquid_send_sats > 0)
                .then(|| {
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
                        .push(
                            text(localized_copy("pending"))
                                .size(P2_SIZE)
                                .style(theme::text::secondary),
                        )
                }),
        )
        .push_maybe(
            (!wallet_balance_masked
                && liquid_balance_loaded
                && !liquid_balance_error
                && pending_liquid_receive_sats > 0)
                .then(|| {
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
                        .push(
                            text(localized_copy("pending"))
                                .size(P2_SIZE)
                                .style(theme::text::secondary),
                        )
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
                    "usdt", "liquid", 32.0,
                ))
                .push(text("USDt").size(P1_SIZE).bold().width(Length::Fixed(60.0)))
                .push(if wallet_balance_masked {
                    Row::new().push(text("********").size(P1_SIZE))
                } else if usdt_balance_error {
                    Row::new().push(
                        text(localized_copy("Balance unavailable"))
                            .size(P1_SIZE)
                            .color(color::RED),
                    )
                } else if !usdt_balance_loaded {
                    Row::new().push(spinner::typing_text_carousel(
                        "...",
                        false,
                        Duration::from_millis(400),
                        |content| text(content).size(P1_SIZE).style(theme::text::secondary),
                    ))
                } else {
                    Row::new()
                        .spacing(6)
                        .align_y(Alignment::Center)
                        .push(text(format_usdt_display(usdt_balance)).size(P1_SIZE).bold())
                        .push(text("USDt").size(P2_SIZE).color(color::GREY_3))
                })
                .push(Space::new().width(Length::Fill))
                .push(
                    landing_button(
                        landing_control_id(cube_id, "Liquid", "send-usdt"),
                        localized_copy("Send"),
                        None,
                        Message::FocusAction,
                        Some(Message::Home(HomeMessage::SendAsset(
                            crate::app::state::liquid::send::SendAsset::Usdt,
                        ))),
                        theme::button::primary,
                    )
                    .width(Length::Fixed(90.0)),
                )
                .push(orange_outline_btn(
                    "Liquid",
                    "receive-usdt",
                    Message::Home(HomeMessage::ReceiveAsset(
                        crate::app::state::liquid::send::SendAsset::Usdt,
                    )),
                )),
        )
        .push_maybe(
            (!wallet_balance_masked
                && usdt_balance_loaded
                && !usdt_balance_error
                && pending_usdt_send_sats > 0)
                .then(|| {
                    Row::new()
                        .spacing(6)
                        .align_y(Alignment::Center)
                        .push(warning_icon().size(12).style(theme::text::secondary))
                        .push(
                            text(localized_format!(
                                "-{} USDt pending",
                                "-{} USDt pendiente",
                                format_usdt_display(pending_usdt_send_sats)
                            ))
                            .size(P2_SIZE)
                            .style(theme::text::secondary),
                        )
                }),
        )
        .push_maybe(
            (!wallet_balance_masked
                && usdt_balance_loaded
                && !usdt_balance_error
                && pending_usdt_receive_sats > 0)
                .then(|| {
                    Row::new()
                        .spacing(6)
                        .align_y(Alignment::Center)
                        .push(warning_icon().size(12).style(theme::text::secondary))
                        .push(
                            text(localized_format!(
                                "+{} USDt pending",
                                "+{} USDt pendiente",
                                format_usdt_display(pending_usdt_receive_sats)
                            ))
                            .size(P2_SIZE)
                            .style(theme::text::secondary),
                        )
                }),
        );

    // Combined Liquid headline (L-BTC + USDt-as-sats). Both component
    // balances must be loaded — a partial sum would be misleading.
    let liquid_card_header_loaded = liquid_balance_loaded && usdt_balance_loaded;
    let liquid_card_header: Element<'a, Message> = if liquid_card_header_loaded
        && (liquid_balance_error
            || usdt_balance_error
            || (usdt_balance > 0 && usable_usd_quote.is_none()))
    {
        amount_cell::<Message>(
            AmountCell::unavailable(coincube_ui::locale::copy(
                "Combined Liquid valuation is partial; asset amounts are below",
            )),
            AmountTier::Card,
        )
    } else if liquid_card_header_loaded {
        wallet_header::<Message>(
            WalletHeaderProps::new(
                total_amount,
                total_fiat,
                wallet_balance_masked,
                bitcoin_unit,
                HeaderVariant::Card,
                display_mode,
                Some(Message::FlipDisplayMode),
            )
            .with_swap_focus(
                landing_control_id(cube_id, "Liquid", "flip-balance"),
                Message::FocusAction,
            ),
        )
        .into()
    } else {
        Column::new()
            .spacing(4)
            .push(spinner::typing_text_carousel(
                "...",
                false,
                Duration::from_millis(400),
                |content| text(content).size(H2_SIZE).bold(),
            ))
            .push(spinner::typing_text_carousel(
                "...",
                false,
                Duration::from_millis(400),
                |content| text(content).size(P1_SIZE).style(theme::text::secondary),
            ))
            .into()
    };

    let liquid_card_content = Column::new()
        .spacing(12)
        .push(landing_button(
            landing_control_id(cube_id, "Liquid", "open"),
            "Liquid",
            Some(droplet_fill_icon().size(16).style(theme::text::secondary)),
            Message::FocusAction,
            Some(Message::Menu(Menu::Liquid(LiquidSubMenu::Overview))),
            theme::button::transparent,
        ))
        .push(liquid_card_header)
        .push(lbtc_row)
        .push(usdt_row);

    let liquid_card = openable_card(
        Container::new(liquid_card_content)
            .padding(20)
            .style(landing_card_style),
        Message::Menu(Menu::Liquid(LiquidSubMenu::Overview)),
    );

    // --- Spark card (BTC row only, mirrors the Liquid layout) ---
    // Rendered above the Liquid card because Spark is the default
    // wallet for everyday Lightning UX post-Phase 5. The card only
    // surfaces when `has_spark` is true — cubes without a Spark
    // signer hide it entirely, so the entire widget tree is built
    // lazily inside `then(..)` to avoid wasted work on every render.
    let spark_card = has_spark.then(|| {
        let spark_fiat = (!spark_valuation_partial)
            .then(|| {
                fiat_converter
                    .as_ref()
                    .map(|c| c.convert(spark_display_balance))
            })
            .flatten();
        let spark_btc_fiat = fiat_converter
            .as_ref()
            .map(|c| c.convert(spark_btc_balance));
        // `fit_row`, as on the Vault card.
        let spark_btc_row = coincube_ui::component::tenshu::fit_row(
            Row::new()
                .spacing(8)
                .align_y(Alignment::Center)
                .push(coincube_ui::image::asset_network_logo::<Message>(
                    "btc", "spark", 32.0,
                ))
                .push(text("BTC").size(P1_SIZE).bold().width(Length::Fixed(60.0)))
                .push(if wallet_balance_masked {
                    Row::new().push(text(AmountCell::masked().primary).size(P1_SIZE))
                } else if spark_balance_error && !spark_balance_loaded {
                    Row::new().push(
                        text(localized_copy("Balance unavailable"))
                            .size(P1_SIZE)
                            .style(theme::text::warning),
                    )
                } else if !spark_balance_loaded {
                    // Pre-Synced placeholder — see the comment on the
                    // Spark wallet_header below for why we don't render
                    // the raw value yet. Animated dots mirror the Total
                    // Balance loading treatment for visual consistency.
                    Row::new().push(spinner::typing_text_carousel(
                        "...",
                        false,
                        Duration::from_millis(400),
                        |content| text(content).size(P1_SIZE).style(theme::text::secondary),
                    ))
                } else {
                    amount_with_size_and_unit(&spark_btc_balance, P1_SIZE, bitcoin_unit)
                })
                .push_maybe(
                    (!wallet_balance_masked && spark_balance_loaded)
                        .then(|| {
                            spark_btc_fiat
                                .as_ref()
                                .map(|f| f.to_text().size(P2_SIZE).style(theme::text::secondary))
                        })
                        .flatten(),
                ),
            Row::new()
                .spacing(8)
                .align_y(Alignment::Center)
                .push(
                    landing_button(
                        landing_control_id(cube_id, "Spark", "send-btc"),
                        localized_copy("Send"),
                        None,
                        Message::FocusAction,
                        Some(Message::Home(HomeMessage::SendSparkBtc)),
                        theme::button::primary,
                    )
                    .width(Length::Fixed(90.0)),
                )
                .push(orange_outline_btn(
                    "Spark",
                    "receive-btc",
                    Message::Home(HomeMessage::ReceiveSparkBtc),
                )),
        );

        // Headline balance — placeholder while the bridge hasn't
        // corroborated the Spark balance with a `Synced` event yet,
        // since the SDK can return a stale persisted value before
        // incremental sync completes. Animated dots match the Total
        // Balance loading treatment for visual consistency.
        let spark_header: Element<'a, Message> = if spark_balance_error && !spark_balance_loaded {
            amount_cell::<Message>(
                AmountCell::unavailable(coincube_ui::locale::copy(
                    "Spark bridge has not returned a balance",
                )),
                AmountTier::Card,
            )
        } else if spark_balance_loaded && spark_valuation_partial {
            amount_cell::<Message>(
                if wallet_balance_masked {
                    AmountCell::masked()
                } else {
                    AmountCell::unavailable(coincube_ui::locale::copy(
                        "Combined BTC and USDB valuation needs a BTC/USD quote",
                    ))
                },
                AmountTier::Card,
            )
        } else if spark_balance_loaded {
            wallet_header::<Message>(
                WalletHeaderProps::new(
                    spark_display_balance,
                    spark_fiat,
                    wallet_balance_masked,
                    bitcoin_unit,
                    HeaderVariant::Card,
                    display_mode,
                    Some(Message::FlipDisplayMode),
                )
                .with_swap_focus(
                    landing_control_id(cube_id, "Spark", "flip-balance"),
                    Message::FocusAction,
                ),
            )
            .into()
        } else {
            Column::new()
                .spacing(4)
                .push(spinner::typing_text_carousel(
                    "...",
                    false,
                    Duration::from_millis(400),
                    |content| text(content).size(H2_SIZE).bold(),
                ))
                .push(spinner::typing_text_carousel(
                    "...",
                    false,
                    Duration::from_millis(400),
                    |content| text(content).size(P1_SIZE).style(theme::text::secondary),
                ))
                .into()
        };

        let spark_card_content = Column::new()
            .spacing(12)
            .push(landing_button(
                landing_control_id(cube_id, "Spark", "open"),
                "Spark",
                Some(spark_icon().size(16).style(theme::text::secondary)),
                Message::FocusAction,
                Some(Message::Menu(Menu::Spark(
                    crate::app::menu::SparkSubMenu::Overview,
                ))),
                theme::button::transparent,
            ))
            .push(spark_header)
            .push(spark_btc_row)
            .push_maybe(spark_usdb_balance.map(|holding| {
                let text_value = format!(
                    "{} USDB",
                    crate::app::breez_spark::assets::format_token_display_u128(
                        holding.balance,
                        holding.decimals,
                    )
                );
                Row::new()
                    .spacing(8)
                    .align_y(Alignment::Center)
                    .push(
                        text("USDB")
                            .size(P1_SIZE)
                            .style(theme::text::secondary)
                            .width(Length::Fixed(60.0)),
                    )
                    .push(amount_cell::<Message>(
                        if wallet_balance_masked {
                            AmountCell::masked()
                        } else if spark_balance_error && !spark_balance_loaded {
                            AmountCell::unavailable(coincube_ui::locale::copy(
                                "Spark bridge has not returned a balance",
                            ))
                        } else if !spark_balance_loaded {
                            AmountCell::unknown("Waiting for Spark sync")
                        } else {
                            AmountCell::shown(text_value, None)
                        },
                        AmountTier::Row,
                    ))
            }))
            .push_maybe((spark_balance_error && spark_balance_loaded).then(|| {
                text(localized_copy("Last known Spark balance · refresh failed"))
                    .size(P2_SIZE)
                    .style(theme::text::warning)
            }))
            .push_maybe(pending_spark_incoming.and_then(|pt| {
                (pt.stage != TransferStage::Completed)
                    .then(|| pending_deposit_card(pt, bitcoin_unit, wallet_balance_masked))
            }));

        openable_card(
            Container::new(spark_card_content)
                .padding(20)
                .style(landing_card_style),
            Message::Menu(Menu::Spark(crate::app::menu::SparkSubMenu::Overview)),
        )
    });

    let vault_card_element: Element<'a, Message> = {
        wallet_card(
            WalletType::Vault,
            cube_id,
            &display_vault_balance,
            fiat_converter,
            wallet_balance_masked,
            has_vault || (has_imported_bitcoin && imported_installed),
            has_vault && recovery_available,
            has_imported_bitcoin && !imported_installed,
            bitcoin_unit,
            pending_vault_incoming,
            vault_pending_send_sats,
            vault_pending_receive_sats,
            display_mode,
            if has_imported_bitcoin {
                imported_balance_loaded
            } else {
                vault_loaded
            },
            vault_alias,
            vault_stale.clone(),
        )
    };

    // Total Balance block — the fiat-first headline number above the
    // Wallets section. Before the first authoritative result, a zero is not
    // a wallet balance. A failed first read without any known component is
    // unavailable; with known components, the sum is explicitly partial.
    let any_known_balance = (has_spark && spark_balance_loaded)
        || (has_liquid
            && ((liquid_balance_loaded && !liquid_balance_error)
                || (usdt_balance_loaded && !usdt_balance_error)))
        || imported_balance_loaded
        || (has_vault && !has_imported_bitcoin && vault_loaded);
    // The Cube and network line is in the page head (`view::page_head`).
    let total_balance_label_row = Row::new()
        .spacing(8)
        .width(Length::Fill)
        .align_y(Alignment::Center)
        .push(h3(localized_copy("Total balance")).bold())
        .push(overview_eye(
            cube_id,
            coincube_ui::locale::copy("Total Balance"),
            overview_masks.total,
            balance_masked,
            HomeMessage::ToggleTotalBalance,
        ))
        .push_maybe(
            (total_partial && !total_balance_loading)
                .then(|| warning_icon().size(12).style(theme::text::secondary)),
        );

    let total_balance_value: Element<'a, Message> = if total_balance_loading {
        Column::new()
            .spacing(4)
            .push(spinner::typing_text_carousel(
                "...",
                false,
                Duration::from_millis(400),
                |content| text(content).size(H1_SIZE).bold(),
            ))
            .push(spinner::typing_text_carousel(
                "...",
                false,
                Duration::from_millis(400),
                |content| text(content).size(P1_SIZE).style(theme::text::secondary),
            ))
            .into()
    } else if !any_known_balance {
        amount_cell::<Message>(
            if total_balance_masked {
                AmountCell::masked()
            } else {
                AmountCell::unavailable(coincube_ui::locale::copy(
                    "No wallet balance has loaded successfully",
                ))
            },
            AmountTier::Total,
        )
    } else {
        wallet_header::<Message>(
            WalletHeaderProps::new(
                global_total_amount,
                global_total_fiat,
                total_balance_masked,
                bitcoin_unit,
                HeaderVariant::Jumbo,
                display_mode,
                Some(Message::FlipDisplayMode),
            )
            .with_swap_focus(
                landing_control_id(cube_id, "Hub", "total-flip-balance"),
                Message::FocusAction,
            ),
        )
        .into()
    };

    let total_balance_block = Column::new()
        .spacing(6)
        .push(total_balance_label_row)
        .push(total_balance_value)
        .push_maybe(
            (total_partial && !total_balance_loading && !total_balance_masked).then(|| {
                text(localized_copy(
                    "Partial total · one or more wallet amounts or USD valuations are unavailable",
                ))
                .size(P2_SIZE)
                .style(theme::text::warning)
            }),
        );

    // A Cube with no wallet yet has nothing to total; the Wallets cards
    // carry the create/restore actions on their own.
    let has_any_wallet = has_spark || has_liquid || has_vault || has_imported_bitcoin;

    let overview = Container::new(
        Column::new()
            .spacing(12)
            .push_maybe(has_any_wallet.then_some(total_balance_block))
            .push_maybe(has_any_wallet.then(|| Space::new().height(Length::Fixed(20.0))))
            .push(
                Row::new()
                    .spacing(8)
                    .width(Length::Fill)
                    .push(h3(localized_copy("Wallets")).bold())
                    .push(overview_eye(
                        cube_id,
                        coincube_ui::locale::copy("Wallets"),
                        overview_masks.wallets,
                        balance_masked,
                        HomeMessage::ToggleWalletBalances,
                    ))
                    .align_y(Alignment::Center),
            )
            // `.cards`: two to a row while the column fits them.
            .push(coincube_ui::component::tenshu::cards(
                spark_card
                    .into_iter()
                    .chain(has_liquid.then_some(liquid_card))
                    .chain(std::iter::once(vault_card_element)),
            )),
    )
    .into();

    (overview, transfer)
}

/// The Overview page (`P-Overview`): its head with the Transfer action, the
/// balances and wallet cards, then "Recent activity" (`.sec`, 36px below).
pub fn overview_page<'a>(
    menu: &'a Menu,
    cache: &'a crate::app::cache::Cache,
    transfer: Option<Element<'a, Message>>,
    overview: Element<'a, Message>,
    recent_activity: Option<Element<'a, Message>>,
) -> Element<'a, Message> {
    let mut head = crate::app::view::page_head(cache, "Overview");
    if let Some(transfer) = transfer {
        head = head.action(transfer);
    }
    let body = Column::new()
        .spacing(36)
        .push_maybe(
            coincube_ui::worldview_copy::VoiceLine::Overview
                .get()
                .map(|line| p2_regular(line).style(theme::text::secondary)),
        )
        .push(overview)
        .push_maybe(recent_activity);
    crate::app::view::dashboard_headed(menu, cache, head, body)
}

/// The Overview head's Transfer action (`P-Overview` `.hact`), present while
/// two wallets can move funds between them. It opens the transfer dialog
/// (#924).
///
/// It stays disabled while any wallet's balance hasn't been corroborated
/// yet. The amount-entry view and wallet picker both consume the raw balance
/// values to render available-funds hints and to cap validation; opening
/// Transfer pre-load would surface a misleading zero / stale cap for
/// whichever wallet hasn't finished syncing.
pub fn transfer_head_action<'a>(config: &GlobalViewConfig<'_>) -> Option<Element<'a, Message>> {
    crate::app::state::default_transfer_direction(
        config.has_liquid,
        config.has_vault,
        config.has_spark,
        config.chain,
    )
    .map(|_| {
        coincube_ui::component::tenshu::transfer_action(
            landing_control_id(config.cube_id, "Hub", "transfer"),
            Message::FocusAction,
            transfer_balance_ready(
                config.total_balance_loading,
                config.has_spark,
                config.spark_balance_loaded,
            )
            .then_some(Message::Home(HomeMessage::NextStep)),
        )
        .compact()
        .into()
    })
}

/// Whether a wallet page's Transfer can start (see
/// `GlobalHome::wallet_transfer_entry`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalletTransferEntry {
    /// The flow can open from this wallet.
    Ready,
    /// The balances the form caps against are still loading.
    Loading,
    /// No other wallet in this Cube and no other Cube can receive.
    NoDestination,
}

/// Why a wallet page's Transfer is disabled when there is nowhere to send.
pub const NO_TRANSFER_DESTINATION: &str =
    "Add another wallet to this Cube, or create another Cube, to transfer between them.";

/// A wallet page's Transfer: the Overview's action, opening the flow with
/// this wallet as the source. With nowhere to send it stays disabled and
/// says, on hover, what would give it a destination.
pub fn wallet_transfer_action<'a>(
    cube_id: &str,
    wallet: WalletKind,
    entry: WalletTransferEntry,
) -> Element<'a, Message> {
    let action: Element<'a, Message> = coincube_ui::component::tenshu::transfer_action(
        landing_control_id(cube_id, wallet.label(), "transfer"),
        Message::FocusAction,
        (entry == WalletTransferEntry::Ready)
            .then_some(Message::Home(HomeMessage::TransferFromWallet(wallet))),
    )
    .compact()
    .into();
    match entry {
        WalletTransferEntry::NoDestination => coincube_ui::component::tooltip::tooltip_custom(
            localized_copy(NO_TRANSFER_DESTINATION),
            action,
            iced::widget::tooltip::Position::Bottom,
        )
        .into(),
        WalletTransferEntry::Ready | WalletTransferEntry::Loading => action,
    }
}

/// Aggregate failure may end loading for display while an installed Spark
/// wallet still has no authoritative funds for the transfer picker or cap.
pub(crate) fn transfer_balance_ready(
    total_balance_loading: bool,
    has_spark: bool,
    spark_balance_loaded: bool,
) -> bool {
    !total_balance_loading && (!has_spark || spark_balance_loaded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::view::shared::landing::capture;

    /// A wallet page's Transfer opens the flow from that wallet when ready,
    /// and does nothing while loading or with nowhere to send.
    #[test]
    fn a_wallet_page_transfer_button_opens_from_its_wallet_only_when_ready() {
        for wallet in [WalletKind::Spark, WalletKind::Vault] {
            let id = landing_control_id("cube", wallet.label(), "transfer");
            for pointer in [false, true] {
                let emitted = capture::activate(
                    wallet_transfer_action("cube", wallet, WalletTransferEntry::Ready),
                    id.clone(),
                    (1024, 768),
                    pointer,
                );
                assert!(emitted.iter().any(|message| matches!(
                    message,
                    Message::Home(HomeMessage::TransferFromWallet(from)) if *from == wallet
                )));
            }
            let ready = capture::inspect(
                "wallet-transfer-ready",
                wallet_transfer_action("cube", wallet, WalletTransferEntry::Ready),
                (1024, 768),
                "dark",
                theme::Theme::dark(),
            );
            assert!(ready.has_focus(id.clone()), "{:?}", wallet);
            // Loading, or nowhere to send: shown, but disabled.
            for entry in [
                WalletTransferEntry::Loading,
                WalletTransferEntry::NoDestination,
            ] {
                let probe = capture::inspect(
                    "wallet-transfer-disabled",
                    wallet_transfer_action("cube", wallet, entry),
                    (1024, 768),
                    "dark",
                    theme::Theme::dark(),
                );
                assert!(probe.contains("Transfer"), "{:?} {:?}", wallet, entry);
                assert!(!probe.has_focus(id.clone()), "{:?} {:?}", wallet, entry);
            }
        }
        assert_eq!(back_to(None), "Overview");
        assert_eq!(back_to(Some(WalletKind::Spark)), "Spark");
        assert_eq!(back_to(Some(WalletKind::Vault)), "Vault");
        // The disabled button's hover copy is translated.
        assert_ne!(
            coincube_ui::locale::render_in(coincube_ui::locale::Language::Spanish, || {
                localized_copy(NO_TRANSFER_DESTINATION).to_owned()
            }),
            NO_TRANSFER_DESTINATION
        );
    }

    /// The Overview a step renders beneath any transfer dialog.
    fn overview(config: GlobalViewConfig<'_>) -> Element<'_, Message> {
        global_home_view(config).0
    }

    /// The Overview's head as the page draws it: its title and its Transfer
    /// action (the Cube line needs a cache, and has no controls).
    fn overview_head(config: GlobalViewConfig<'_>) -> Element<'static, Message> {
        let mut head = coincube_ui::component::tenshu::page_head("Overview", Message::FocusAction);
        if let Some(transfer) = transfer_head_action(&config) {
            head = head.action(transfer);
        }
        head.view(crate::app::view::page_gutter)
    }

    /// The dialog a transfer step (1–3) lays over the Overview.
    /// A transfer step as the shell shows it: the page head (as
    /// [`transfer_page_head`] builds it from the Cube) above the body.
    fn transfer_dialog_for(config: GlobalViewConfig<'_>) -> Element<'_, Message> {
        let context = format!("{} · {}", config.cube_name, network_name(config.chain));
        let screen = global_home_view(config)
            .1
            .expect("a transfer step renders its page");
        let head = coincube_ui::component::tenshu::page_head("Transfer", Message::FocusAction)
            .context(Space::new(), context)
            .rule(transfer_steps_row(screen.step));
        let head = if screen.can_leave {
            head.back(screen.back_to, Message::Home(HomeMessage::LeaveTransfer))
        } else {
            head
        };
        // The shell's page column pads the body to the head's gutter.
        let gutter = crate::app::view::page_gutter(1192.0);
        Column::new()
            .push(head.view(crate::app::view::page_gutter))
            .push(Container::new(screen.body).padding(iced::Padding {
                top: 24.0,
                bottom: 24.0,
                left: gutter,
                right: gutter,
            }))
            .into()
    }
    use crate::{
        app::{
            cache::FiatPriceRequest,
            settings::{display::DisplayMode, unit::BitcoinDisplayUnit},
            view::shared::feerate_picker::FeeratePreset,
        },
        services::fiat::{Currency, PriceSource},
    };
    use coincube_core::miniscript::bitcoin::{bip32::ChildNumber, Address};
    use std::{collections::HashMap, str::FromStr};

    const MAINNET_ADDR: &str = "bc1qvrl2849aggm6qry9ea7xqp2kk39j8vaa8r3cwg";

    #[test]
    fn pending_deposit_card_follows_passive_privacy_on_both_home_wallets() {
        let transfer = PendingTransfer {
            amount: Amount::from_sat(98_765_432),
            stage: TransferStage::PendingDeposit,
        };
        let inspect = |masked| {
            capture::inspect(
                "home-pending-deposit",
                pending_deposit_card(transfer, BitcoinDisplayUnit::Sats, masked),
                (900, 200),
                "dark",
                theme::Theme::dark(),
            )
        };
        let hidden = inspect(true);
        assert!(hidden.contains("••••••"));
        assert!(!hidden.contains("98,765,432"));
        let shown = inspect(false);
        assert!(shown.contains("98,765,432"));
    }

    struct ViewFixture {
        entered_amount: form::Value<String>,
        invalid_amount: form::Value<String>,
        transfer_feerate: form::Value<String>,
        invalid_feerate: form::Value<String>,
        receive_address: Address,
        receive_index: ChildNumber,
        labels: HashMap<String, String>,
        labels_editing: HashMap<String, form::Value<String>>,
        fiat_converter: FiatAmountConverter,
    }

    impl ViewFixture {
        fn new() -> Self {
            let receive_address = Address::from_str(MAINNET_ADDR).unwrap().assume_checked();
            let mut labels = HashMap::new();
            labels.insert(receive_address.to_string(), "Vault receive".to_string());

            Self {
                entered_amount: form::Value {
                    value: "50000".to_string(),
                    valid: true,
                    warning: None,
                },
                invalid_amount: form::Value {
                    value: "not sats".to_string(),
                    valid: false,
                    warning: Some("Invalid amount format"),
                },
                transfer_feerate: form::Value {
                    value: "3".to_string(),
                    valid: true,
                    warning: None,
                },
                invalid_feerate: form::Value {
                    value: "0".to_string(),
                    valid: false,
                    warning: Some("Feerate must be positive"),
                },
                receive_address,
                receive_index: ChildNumber::from(0),
                labels,
                labels_editing: HashMap::new(),
                fiat_converter: FiatAmountConverter::new(
                    50_000.0,
                    Some(1_722_000_000),
                    FiatPriceRequest::new(PriceSource::Coincube, Currency::USD),
                )
                .unwrap(),
            }
        }

        fn config(
            &self,
            step: usize,
            direction: Option<TransferDirection>,
            wallet_picker: Option<PickerSide>,
        ) -> GlobalViewConfig<'_> {
            GlobalViewConfig {
                cube_id: "test-cube",
                liquid_balance: Amount::from_sat(125_000),
                spark_balance: Amount::from_sat(250_000),
                spark_btc_balance: Amount::from_sat(250_000),
                spark_usdb_balance: None,
                spark_balance_error: false,
                usdt_balance: 100_000_000,
                usdt_balance_error: false,
                pending_liquid_send_sats: 1_000,
                pending_usdt_send_sats: 2_000,
                pending_liquid_receive_sats: 3_000,
                pending_usdt_receive_sats: 4_000,
                vault_pending_send_sats: 5_000,
                vault_pending_receive_sats: 6_000,
                vault_balance: Amount::from_sat(375_000),
                imported_balance: None,
                imported_installed: false,
                fiat_converter: Some(self.fiat_converter),
                balance_masked: false,
                overview_total_hidden: false,
                overview_wallets_hidden: false,
                total_balance_loading: false,
                spark_balance_loaded: true,
                liquid_balance_loaded: true,
                liquid_balance_error: false,
                usdt_balance_loaded: true,
                vault_loaded: true,
                recovery_available: true,
                vault_stale: None,
                display_mode: DisplayMode::FiatNative,
                has_vault: true,
                has_imported_bitcoin: false,
                has_spark: true,
                has_liquid: true,
                current_view: HomeView { step },
                transfer_direction: direction,
                transfer_from: direction.map(|d| d.from_kind()),
                transfer_to: direction.map(|d| d.to_kind()),
                wallet_picker,
                entered_amount: &self.entered_amount,
                receive_address: Some(&self.receive_address),
                destination_ready: true,
                transfer_action_ready: true,
                receive_index: Some(&self.receive_index),
                labels: &self.labels,
                labels_editing: &self.labels_editing,
                address_expanded: false,
                bitcoin_unit: BitcoinDisplayUnit::Sats,
                onchain_send_limit: Some((25_000, 5_000_000)),
                onchain_receive_limit: Some((25_000, 5_000_000)),
                is_sending: false,
                is_tx_signed: false,
                prepare_onchain_send_response: None,
                spend_tx_fees: None,
                transfer_feerate: &self.transfer_feerate,
                transfer_feerate_loading: None,
                spark_send_fee_sat: None,
                pending_spark_incoming: Some(PendingTransfer {
                    amount: Amount::from_sat(31_000),
                    stage: TransferStage::PendingDeposit,
                }),
                pending_vault_incoming: Some(PendingTransfer {
                    amount: Amount::from_sat(41_000),
                    stage: TransferStage::PendingDeposit,
                }),
                btc_usd_price: Some(50_000.0),
                cube_name: "Everyday",
                vault_alias: Some("Family savings"),
                chain: ChainId::Bitcoin,
                light_theme: false,
                transfer_page: TransferPage::Wallets,
                transfer_unit: BitcoinDisplayUnit::Sats,
                transfer_max: false,
                transfer_origin: None,
                other_cube: OtherCubeView::default(),
            }
        }
    }

    fn directions() -> [TransferDirection; 6] {
        use TransferDirection::*;
        [
            LiquidToVault,
            VaultToLiquid,
            LiquidToSpark,
            SparkToLiquid,
            VaultToSpark,
            SparkToVault,
        ]
    }

    #[test]
    fn hub_mask_redacts_pending_deposit_amount() {
        let pending = PendingTransfer {
            amount: Amount::from_sat(31_000),
            stage: TransferStage::PendingDeposit,
        };
        let visible = pending_deposit_amount(pending, BitcoinDisplayUnit::Sats, false);
        let hidden = pending_deposit_amount(pending, BitcoinDisplayUnit::Sats, true);
        assert!(visible.contains("31"));
        assert!(visible.contains("SATS"));
        assert_eq!(hidden, AmountCell::masked().primary);
        assert!(!hidden.contains("31"));
    }

    #[test]
    fn r06a_hub_headless_content_matrix_and_presync_usdb_gate() {
        use crate::app::view::shared::landing::capture;
        let fixture = ViewFixture::new();
        let usdb = coincube_spark_protocol::UsdbBalanceSnapshot {
            token_identifier: "btkn1synthetic".into(),
            balance: 93_420_000_000,
            decimals: 6,
            ticker: "USDB".into(),
        };
        let raw_usdb = format!(
            "{} USDB",
            crate::app::breez_spark::assets::format_token_display_u128(usdb.balance, usdb.decimals)
        );
        for (appearance, theme) in [
            ("dark", theme::Theme::dark()),
            ("light", theme::Theme::light()),
        ] {
            for outer in [(1024, 768), (1440, 900)] {
                let normal = capture::inspect(
                    "hub-normal",
                    overview(fixture.config(0, None, None)),
                    outer,
                    appearance,
                    theme,
                );
                assert!(normal.contains("Total balance"));
                assert!(!normal.contains("Total Balance"));
                assert!(normal.contains("Wallets"));
                // Transfer is the head's action now, not a button under the
                // cards.
                assert!(!normal.has_focus(landing_control_id("test-cube", "Hub", "transfer")));
                for (wallet, control) in [
                    ("Spark", "open"),
                    ("Spark", "send-btc"),
                    ("Spark", "receive-btc"),
                    ("Liquid", "open"),
                    ("Liquid", "send-lbtc"),
                    ("Vault", "open"),
                    ("Vault", "send-btc"),
                ] {
                    assert!(normal.has_focus(landing_control_id("test-cube", wallet, control)));
                }

                let mut presync = fixture.config(0, None, None);
                presync.spark_usdb_balance = Some(&usdb);
                presync.spark_balance_loaded = false;
                presync.total_balance_loading = true;
                let before = capture::inspect(
                    "hub-presync-usdb",
                    overview(presync),
                    outer,
                    appearance,
                    theme,
                );
                assert!(!before.contains(&raw_usdb));
                assert!(before.contains("Waiting for Spark sync"));

                let mut first_error = fixture.config(0, None, None);
                first_error.spark_usdb_balance = Some(&usdb);
                first_error.spark_balance_loaded = false;
                first_error.spark_balance_error = true;
                let failed = capture::inspect(
                    "hub-presync-error",
                    overview(first_error),
                    outer,
                    appearance,
                    theme,
                );
                assert!(!failed.contains(&raw_usdb));
                assert!(failed.contains("Balance unavailable"));

                let mut trusted = fixture.config(0, None, None);
                trusted.spark_usdb_balance = Some(&usdb);
                trusted.spark_balance_loaded = true;
                trusted.btc_usd_price = None;
                let shown = capture::inspect(
                    "hub-usdb-missing-quote",
                    overview(trusted),
                    outer,
                    appearance,
                    theme,
                );
                assert!(shown.contains(&raw_usdb));
                assert!(shown.contains("Partial total"));

                let mut masked = fixture.config(0, None, None);
                masked.spark_usdb_balance = Some(&usdb);
                masked.balance_masked = true;
                let hidden =
                    capture::inspect("hub-masked", overview(masked), outer, appearance, theme);
                assert!(!hidden.contains(&raw_usdb));
                assert!(!hidden.contains("31,000"));

                let mut absent_vault = fixture.config(0, None, None);
                absent_vault.has_vault = false;
                absent_vault.has_imported_bitcoin = true;
                absent_vault.imported_installed = false;
                let import = capture::inspect(
                    "hub-import-pending",
                    overview(absent_vault),
                    outer,
                    appearance,
                    theme,
                );
                assert!(
                    import.contains("Wallet import pending") || import.contains("Create Vault")
                );

                let mut known_zero = fixture.config(0, None, None);
                known_zero.liquid_balance = Amount::ZERO;
                known_zero.spark_btc_balance = Amount::ZERO;
                known_zero.spark_balance = Amount::ZERO;
                known_zero.usdt_balance = 0;
                known_zero.vault_balance = Amount::ZERO;
                let zero = capture::inspect(
                    "hub-known-zero",
                    overview(known_zero),
                    outer,
                    appearance,
                    theme,
                );
                assert!(!zero.contains("No wallet balance has loaded successfully"));

                let mut empty = fixture.config(0, None, None);
                empty.has_vault = false;
                empty.has_spark = false;
                empty.has_liquid = false;
                empty.has_imported_bitcoin = false;
                let no_wallets =
                    capture::inspect("hub-no-wallets", overview(empty), outer, appearance, theme);
                assert!(!no_wallets.contains("Total balance"));
                assert!(!no_wallets.contains("No wallet balance has loaded successfully"));
                assert!(no_wallets.contains("Create Vault"));
            }
        }
    }

    #[test]
    fn imported_bitcoin_counts_only_after_an_installed_authoritative_balance() {
        use crate::app::view::shared::landing::capture;

        let fixture = ViewFixture::new();
        let inspect = |config| {
            capture::inspect(
                "hub-import-balance",
                overview(config),
                (1024, 768),
                "dark",
                theme::Theme::dark(),
            )
        };
        let imported = || {
            let mut config = fixture.config(0, None, None);
            config.has_vault = false;
            config.has_spark = false;
            config.has_liquid = false;
            config.has_imported_bitcoin = true;
            config.imported_installed = true;
            config.imported_balance = Some(Amount::from_sat(375_000));
            config
        };
        let known = inspect(imported());
        assert!(!known.contains("No wallet balance has loaded successfully"));

        let mut known_zero_config = imported();
        known_zero_config.imported_balance = Some(Amount::ZERO);
        known_zero_config.vault_balance = Amount::ZERO;
        let known_zero = inspect(known_zero_config);
        assert!(!known_zero.contains("No wallet balance has loaded successfully"));

        let mut pending_config = imported();
        pending_config.imported_balance = None;
        let pending = inspect(pending_config);
        assert!(pending.contains("No wallet balance has loaded successfully"));

        let mut uninstalled_config = imported();
        uninstalled_config.imported_installed = false;
        let uninstalled = inspect(uninstalled_config);
        assert!(uninstalled.contains("No wallet balance has loaded successfully"));

        let mut mixed_known_config = imported();
        mixed_known_config.has_vault = true;
        mixed_known_config.vault_loaded = false;
        let mixed_known = inspect(mixed_known_config);
        assert!(!mixed_known.contains("No wallet balance has loaded successfully"));

        let mut mixed_pending_config = imported();
        mixed_pending_config.has_vault = true;
        mixed_pending_config.imported_balance = None;
        mixed_pending_config.vault_loaded = true;
        let mixed_pending = inspect(mixed_pending_config);
        assert!(mixed_pending.contains("No wallet balance has loaded successfully"));

        let mut partial_config = imported();
        partial_config.has_spark = true;
        partial_config.spark_balance_loaded = false;
        partial_config.spark_balance_error = true;
        let partial = inspect(partial_config);
        assert!(partial.contains("Partial total"));
    }

    #[test]
    fn r06a_hub_wallet_header_pointer_and_enter_route_once_to_selected_wallet() {
        use crate::app::view::shared::landing::capture;
        let fixture = ViewFixture::new();
        for pointer in [false, true] {
            for (wallet, menu) in [
                (
                    "Spark",
                    Menu::Spark(crate::app::menu::SparkSubMenu::Overview),
                ),
                ("Liquid", Menu::Liquid(LiquidSubMenu::Overview)),
            ] {
                let messages = capture::activate(
                    overview(fixture.config(0, None, None)),
                    landing_control_id("test-cube", wallet, "open"),
                    (1024, 768),
                    pointer,
                );
                let routes = messages
                    .iter()
                    .filter(|message| matches!(message, Message::Menu(_)))
                    .collect::<Vec<_>>();
                assert_eq!(
                    routes.len(),
                    1,
                    "{wallet} pointer={pointer}: {messages:?}",
                    wallet = wallet,
                    pointer = pointer,
                    messages = messages
                );
                assert!(
                    matches!(routes[0], Message::Menu(actual) if actual == &menu),
                    "{wallet} pointer={pointer}: {messages:?}",
                    wallet = wallet,
                    pointer = pointer,
                    messages = messages
                );
            }
        }
    }

    #[test]
    fn overview_wallet_card_surface_routes_once_to_its_wallet() {
        use crate::app::view::shared::landing::capture;
        let fixture = ViewFixture::new();
        // Plain labels on each card's surface, not on any Action: Spark's
        // and the Vault's "BTC" rows (in card order) and Liquid's "L-BTC".
        for (needle, nth, menu) in [
            (
                "BTC",
                0,
                Menu::Spark(crate::app::menu::SparkSubMenu::Overview),
            ),
            ("L-BTC", 0, Menu::Liquid(LiquidSubMenu::Overview)),
            ("BTC", 1, Menu::Vault(VaultSubMenu::Overview)),
        ] {
            let messages = capture::click_text(
                overview(fixture.config(0, None, None)),
                needle,
                nth,
                // Tall enough that the Vault card, alone on the second
                // row, sits inside the scroll viewport.
                (1024, 2000),
            );
            let routes = messages
                .iter()
                .filter(|message| matches!(message, Message::Menu(_)))
                .collect::<Vec<_>>();
            assert_eq!(
                routes.len(),
                1,
                "{needle} #{nth}: {messages:?}",
                needle = needle,
                nth = nth,
                messages = messages
            );
            assert!(
                matches!(routes[0], Message::Menu(actual) if actual == &menu),
                "{needle} #{nth}: {messages:?}",
                needle = needle,
                nth = nth,
                messages = messages
            );
        }

        // A button on a card keeps its own action; the card does not open
        // its wallet underneath it.
        let messages = capture::activate(
            overview(fixture.config(0, None, None)),
            landing_control_id("test-cube", "Spark", "send-btc"),
            (1024, 768),
            true,
        );
        assert!(
            messages
                .iter()
                .any(|message| matches!(message, Message::Home(HomeMessage::SendSparkBtc))),
            "{messages:?}",
            messages = messages
        );
        assert!(
            !messages
                .iter()
                .any(|message| matches!(message, Message::Menu(_))),
            "{messages:?}",
            messages = messages
        );
    }

    #[test]
    fn overview_vault_recovery_footer_routes_to_tools_and_respects_availability_and_privacy() {
        use crate::app::view::shared::landing::capture;

        let fixture = ViewFixture::new();
        let target = landing_control_id("test-cube", "Vault", "recovery-path");
        for (appearance, theme) in [
            ("dark", theme::Theme::dark()),
            ("light", theme::Theme::light()),
        ] {
            for outer in [(1024, 768), (1440, 900)] {
                let normal = capture::inspect(
                    "overview-vault-recovery",
                    overview(fixture.config(0, None, None)),
                    outer,
                    appearance,
                    theme,
                );
                assert!(normal.contains("Recovery path"));
                assert!(normal.has_focus(target.clone()));

                let mut locked = fixture.config(0, None, None);
                locked.recovery_available = false;
                let locked = capture::inspect(
                    "overview-vault-recovery-locked",
                    overview(locked),
                    outer,
                    appearance,
                    theme,
                );
                assert!(!locked.contains("Recovery path"));
                assert!(!locked.has_focus(target.clone()));

                let mut masked = fixture.config(0, None, None);
                masked.balance_masked = true;
                let hidden = capture::inspect(
                    "overview-vault-recovery-masked",
                    overview(masked),
                    outer,
                    appearance,
                    theme,
                );
                assert!(hidden.contains("Recovery path"));
                assert!(!hidden.contains("250,000"));
                assert!(!hidden.contains("375,000"));

                let mut imported_only = fixture.config(0, None, None);
                imported_only.has_vault = false;
                imported_only.has_imported_bitcoin = true;
                imported_only.imported_installed = true;
                let imported = capture::inspect(
                    "overview-imported-without-vault",
                    overview(imported_only),
                    outer,
                    appearance,
                    theme,
                );
                assert!(!imported.has_focus(target.clone()));
            }

            // A tall content-only frame exposes the below-fold footer for
            // visual review while the standard compact and wide frames above
            // continue to catch width overflow and privacy regressions.
            let full_card = capture::inspect(
                "overview-vault-recovery-full-card",
                overview(fixture.config(0, None, None)),
                (1440, 1400),
                appearance,
                theme,
            );
            assert!(full_card.contains("Recovery path"));
        }

        for pointer in [false, true] {
            let messages = capture::activate(
                overview(fixture.config(0, None, None)),
                target.clone(),
                // The footer is below the fold at the minimum window size;
                // this viewport exposes it for the pointer probe.
                if pointer { (1440, 1400) } else { (1024, 768) },
                pointer,
            );
            let routes = messages
                .iter()
                .filter(|message| matches!(message, Message::Menu(_)))
                .collect::<Vec<_>>();
            assert_eq!(routes.len(), 1, "pointer={pointer}: {messages:?}");
            assert!(matches!(
                routes[0],
                Message::Menu(Menu::Vault(VaultSubMenu::Recovery))
            ));
        }
    }

    #[derive(Clone, Copy, Debug)]
    enum TransferTrigger {
        Pointer,
        Enter,
        Space,
    }

    /// Send real pointer/key events to the Overview's head. Disabled
    /// Actions do not expose focus IDs, so pointer targeting uses the rendered
    /// Transfer text bounds rather than assuming a focusable Action exists.
    fn activate_transfer(
        content: Element<'_, Message>,
        text_bounds: iced::Rectangle,
        trigger: TransferTrigger,
    ) -> Vec<Message> {
        use coincube_ui::font;
        use iced::advanced::{
            layout,
            renderer::Headless,
            widget::{operation, operation::focusable, Operation, Tree},
            Layout,
        };
        use iced::{keyboard, mouse, Event, Rectangle, Size};

        for bytes in font::load() {
            iced::advanced::graphics::text::font_system()
                .write()
                .unwrap()
                .load_font(bytes);
        }
        // The capture's 953px pane at 1440×900, so the label's bounds from
        // the capture land on the action.
        let size = Size::new(953.0, 2400.0);
        assert!(
            text_bounds.y >= 0.0 && text_bounds.y + text_bounds.height <= size.height,
            "Transfer label at {:?} is outside activation viewport",
            text_bounds
        );
        let renderer = iced::futures::executor::block_on(<iced::Renderer as Headless>::new(
            font::REGULAR,
            iced::Pixels(16.0),
            Some("tiny-skia"),
        ))
        .unwrap();
        let mut element: Element<'_, Message> = iced::widget::scrollable(content)
            .width(Length::Fill)
            .height(Length::Fill)
            .into();
        let mut tree = Tree::new(element.as_widget());
        let node = element.as_widget_mut().layout(
            &mut tree,
            &renderer,
            &layout::Limits::new(Size::ZERO, size),
        );
        if !matches!(trigger, TransferTrigger::Pointer) {
            let id = landing_control_id("test-cube", "Hub", "transfer");
            let mut operation: Box<dyn Operation<()>> = Box::new(focusable::focus(id));
            loop {
                element.as_widget_mut().operate(
                    &mut tree,
                    Layout::new(&node),
                    &renderer,
                    &mut operation::black_box(operation.as_mut()),
                );
                match operation.finish() {
                    operation::Outcome::None | operation::Outcome::Some(()) => break,
                    operation::Outcome::Chain(next) => operation = next,
                }
            }
        }
        let cursor = if matches!(trigger, TransferTrigger::Pointer) {
            mouse::Cursor::Available(text_bounds.center())
        } else {
            mouse::Cursor::Unavailable
        };
        let events = match trigger {
            TransferTrigger::Pointer => vec![
                Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
                Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
            ],
            TransferTrigger::Enter | TransferTrigger::Space => {
                let named = if matches!(trigger, TransferTrigger::Enter) {
                    keyboard::key::Named::Enter
                } else {
                    keyboard::key::Named::Space
                };
                let key = keyboard::Key::Named(named);
                let physical_key =
                    keyboard::key::Physical::Unidentified(keyboard::key::NativeCode::Unidentified);
                vec![
                    Event::Keyboard(keyboard::Event::KeyPressed {
                        key: key.clone(),
                        modified_key: key.clone(),
                        physical_key,
                        location: keyboard::Location::Standard,
                        modifiers: keyboard::Modifiers::empty(),
                        text: None,
                        repeat: false,
                    }),
                    Event::Keyboard(keyboard::Event::KeyReleased {
                        key: key.clone(),
                        modified_key: key,
                        physical_key,
                        location: keyboard::Location::Standard,
                        modifiers: keyboard::Modifiers::empty(),
                    }),
                ]
            }
        };
        let mut messages = Vec::new();
        for event in events {
            element.as_widget_mut().update(
                &mut tree,
                &event,
                Layout::new(&node),
                cursor,
                &renderer,
                &mut iced::advanced::clipboard::Null,
                &mut iced::advanced::Shell::new(&mut messages),
                &Rectangle::with_size(size),
            );
        }
        messages
    }

    #[test]
    fn r06a_transfer_action_requires_authoritative_spark_after_partial_total_settles() {
        use crate::app::{state::GlobalHome, view::shared::landing::capture};

        let fixture = ViewFixture::new();
        let snapshots = GlobalHome::r06a_transfer_snapshots();
        for snapshot in &snapshots {
            let mut config = fixture.config(0, None, None);
            // The adapter itself is inert (no bridge). Its real update results
            // feed this installed-Spark pure presenter without fabricating a
            // backend or claiming that State::view had a live Spark route.
            config.has_spark = snapshot.has_spark;
            config.has_liquid = !snapshot.has_spark;
            config.has_vault = true;
            config.spark_balance = snapshot.spark_balance;
            config.spark_btc_balance = snapshot.spark_balance;
            config.spark_balance_loaded = snapshot.spark_balance_loaded;
            config.spark_balance_error = snapshot.spark_balance_error;
            config.total_balance_loading = snapshot.total_balance_loading;
            let inspection = capture::inspect(
                &format!("hub-transfer-{}", snapshot.name),
                overview_head(config),
                (1440, 900),
                "dark",
                theme::Theme::dark(),
            );
            let text_bounds = inspection
                .text
                .iter()
                .find_map(|(value, bounds)| (value == "Transfer").then_some(*bounds))
                .expect("rendered head Transfer label");
            let expected = matches!(
                snapshot.name,
                "trusted-postsync" | "trusted-refresh-error" | "no-spark"
            );
            assert_eq!(
                inspection.has_focus(landing_control_id("test-cube", "Hub", "transfer")),
                expected,
                "{} focus eligibility",
                snapshot.name
            );
            for trigger in [
                TransferTrigger::Pointer,
                TransferTrigger::Enter,
                TransferTrigger::Space,
            ] {
                let mut config = fixture.config(0, None, None);
                config.has_spark = snapshot.has_spark;
                config.has_liquid = !snapshot.has_spark;
                config.has_vault = true;
                config.spark_balance = snapshot.spark_balance;
                config.spark_btc_balance = snapshot.spark_balance;
                config.spark_balance_loaded = snapshot.spark_balance_loaded;
                config.spark_balance_error = snapshot.spark_balance_error;
                config.total_balance_loading = snapshot.total_balance_loading;
                let messages = activate_transfer(overview_head(config), text_bounds, trigger);
                let next_steps = messages
                    .iter()
                    .filter(|message| matches!(message, Message::Home(HomeMessage::NextStep)))
                    .count();
                assert_eq!(
                    next_steps,
                    usize::from(expected),
                    "{} {trigger:?}: {messages:?}",
                    snapshot.name
                );
            }
        }

        let trusted = snapshots
            .iter()
            .find(|snapshot| snapshot.name == "trusted-postsync")
            .unwrap();
        assert_eq!(trusted.spark_balance, Amount::from_sat(12_000));
        let mut amount = fixture.config(1, Some(TransferDirection::SparkToVault), None);
        amount.spark_balance = trusted.spark_balance;
        let entered = capture::inspect(
            "hub-transfer-trusted-amount",
            transfer_dialog_for(amount),
            (1440, 900),
            "dark",
            theme::Theme::dark(),
        );
        assert!(entered.contains("12,000"));
        // The From menu keeps the corroborated Spark amount visible.
        let mut from = fixture.config(1, Some(TransferDirection::SparkToVault), None);
        from.spark_balance = trusted.spark_balance;
        let selected = inspect_menu("hub-transfer-trusted-source", from_menu(&menu_ctx(&from)));
        assert!(selected.contains("Spark"));
        assert!(selected.contains("12,000"));
    }

    #[test]
    fn transfer_wallet_rows_follow_global_balance_mask() {
        use crate::app::view::shared::landing::capture;

        let fixture = ViewFixture::new();
        let direction = Some(TransferDirection::LiquidToSpark);
        let render_amount = |masked| {
            let mut config = fixture.config(1, direction, None);
            config.balance_masked = masked;
            capture::inspect(
                "hub-transfer-privacy-amount",
                transfer_dialog_for(config),
                (1440, 900),
                "dark",
                theme::Theme::dark(),
            )
        };
        let hidden = render_amount(true);
        assert!(hidden.contains("••••••"));
        assert!(!hidden.contains("125,000"));
        assert!(!hidden.contains("250,000"));
        let shown = render_amount(false);
        assert!(shown.contains("125,000"));
        assert!(shown.contains("250,000"));

        let render_source_rows = |masked| {
            let mut config = fixture.config(1, direction, None);
            config.balance_masked = masked;
            config.vault_balance = Amount::from_sat(375_000);
            inspect_menu("hub-transfer-privacy-source", from_menu(&menu_ctx(&config)))
        };
        let hidden_picker = render_source_rows(true);
        assert!(hidden_picker.contains("••••••"));
        for balance in ["125,000", "250,000", "375,000"] {
            assert!(
                !hidden_picker.contains(balance),
                "hidden picker balance {}",
                balance
            );
        }
        let shown_picker = render_source_rows(false);
        for balance in ["125,000", "250,000", "375,000"] {
            assert!(
                shown_picker.contains(balance),
                "shown picker balance {}",
                balance
            );
        }
    }

    /// Transfer is a page, as Send is: Back to Overview in its head
    /// cancels it, except while a send is in flight.
    #[test]
    fn transfer_is_a_page_with_back_to_overview() {
        let fixture = ViewFixture::new();
        // Details, Review, then Sent with every step done.
        for (step, active) in [(1, 0), (2, 1), (3, TRANSFER_STEPS.len())] {
            let screen =
                global_home_view(fixture.config(step, Some(TransferDirection::SparkToVault), None))
                    .1
                    .expect("a transfer step renders its page");
            assert!(screen.can_leave, "step {}", step);
            assert_eq!(screen.step, active);
            let page = inspect_dialog(
                "transfer-page",
                fixture.config(step, Some(TransferDirection::SparkToVault), None),
            );
            assert!(page.contains("Back to Overview"), "step {}", step);
        }
        let mut sending = fixture.config(2, Some(TransferDirection::SparkToVault), None);
        sending.is_sending = true;
        let screen = global_home_view(sending).1.expect("review");
        assert!(!screen.can_leave, "no way out while a send is in flight");
        assert!(global_home_view(fixture.config(0, None, None)).1.is_none());
    }

    #[test]
    fn transfer_form_layout_and_synthetic_capture_matrix() {
        let fixture = ViewFixture::new();
        for (width, height) in [(1024, 768), (1440, 900)] {
            for (appearance, theme) in [
                ("dark", theme::Theme::dark()),
                ("light", theme::Theme::light()),
            ] {
                let snapshot = capture::inspect(
                    "transfer-wallets-amount",
                    transfer_dialog_for(fixture.config(
                        1,
                        Some(TransferDirection::SparkToVault),
                        None,
                    )),
                    (width, height),
                    appearance,
                    theme,
                );
                for label in ["Transfer details", "From", "To", "Prepare"] {
                    assert!(
                        snapshot.contains(label),
                        "{} {}: {}",
                        appearance,
                        width,
                        label
                    );
                }
                assert!(snapshot.contains("250,000"));
                assert!(snapshot.contains("375,000"));
                assert!(snapshot.contains("Available in Everyday"));
            }
        }
    }

    #[test]
    fn transfer_wallet_rows_activate_by_enter_and_space() {
        use iced::keyboard::key::Named;

        let fixture = ViewFixture::new();
        for key in [Named::Enter, Named::Space] {
            let config = fixture.config(1, Some(TransferDirection::SparkToVault), None);
            let content = from_menu(&menu_ctx(&config));
            let messages = capture::activate_with_key(
                content,
                iced::advanced::widget::Id::new(from_option_id(WalletKind::Vault)),
                (1024, 768),
                false,
                key,
            );
            assert_eq!(
                messages
                    .iter()
                    .filter(|message| {
                        matches!(
                            message,
                            Message::Home(HomeMessage::SelectTransferWallet(
                                PickerSide::From,
                                WalletKind::Vault
                            ))
                        )
                    })
                    .count(),
                1,
            );
            assert_eq!(fixture.entered_amount.value, "50000");
        }
    }

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

    #[test]
    fn wallet_kind_labels_and_direction_display_are_stable() {
        assert_eq!(WalletKind::Liquid.label(), "Liquid");
        assert_eq!(WalletKind::Spark.label(), "Spark");
        assert_eq!(WalletKind::Vault.label(), "Vault");

        for direction in directions() {
            assert_eq!(direction.from_wallet(), direction.from_kind().label());
            assert_eq!(direction.to_wallet(), direction.to_kind().label());
            assert_eq!(
                direction.display(),
                format!("{} → {}", direction.from_wallet(), direction.to_wallet())
            );
        }
    }

    #[test]
    fn home_view_step_state_moves_forward_back_and_resets() {
        let mut view = HomeView::default();

        view.previous();
        assert_eq!(view.step, 0);

        view.next();
        view.next();
        assert_eq!(view.step, 2);

        view.previous();
        assert_eq!(view.step, 1);

        view.reset();
        assert_eq!(view.step, 0);
    }

    #[test]
    fn overview_section_masks_never_override_global_hide() {
        assert_eq!(
            OverviewMasks::new(false, false, false),
            OverviewMasks {
                total: false,
                wallets: false
            }
        );
        assert_eq!(
            OverviewMasks::new(false, true, false),
            OverviewMasks {
                total: true,
                wallets: false
            }
        );
        assert_eq!(
            OverviewMasks::new(false, false, true),
            OverviewMasks {
                total: false,
                wallets: true
            }
        );
        for total in [false, true] {
            for wallets in [false, true] {
                assert_eq!(
                    OverviewMasks::new(true, total, wallets),
                    OverviewMasks {
                        total: true,
                        wallets: true
                    }
                );
            }
        }
    }

    /// The Overview body's width in a window `window` wide: the pane beside
    /// the side menu, then the page column inside its gutters
    /// (`dashboard_headed`).
    fn overview_column(window: f32) -> f32 {
        let pane = window - crate::app::view::nav::split::sidebar_width_for_window(window);
        coincube_ui::component::tenshu::content_frame(
            pane,
            coincube_ui::component::tenshu::PageWidth::Reading,
            crate::app::view::page_gutter(pane),
        )
        .1
    }

    /// `.cards` at the two required window sizes: side by side in equal
    /// columns at 1440×900, stacked at the 1024×768 minimum. Liquid is hidden
    /// in v175, so Spark and the Vault share a row.
    #[test]
    fn wallet_cards_sit_side_by_side_at_1440_and_stack_at_1024() {
        use coincube_ui::component::tenshu::{cards::two_columns, CARD_GAP};
        let fixture = ViewFixture::new();
        for (appearance, theme) in [
            ("dark", theme::Theme::dark()),
            ("light", theme::Theme::light()),
        ] {
            for outer in [(1024, 768), (1440, 900)] {
                let width = overview_column(outer.0 as f32).floor();
                let mut config = fixture.config(0, None, None);
                config.has_liquid = false;
                let shown = capture::inspect_wide_content(
                    "overview-cards",
                    overview(config),
                    outer,
                    appearance,
                    theme,
                    width as u32,
                );
                let at = |wallet: &str, control: &str| {
                    let id = landing_control_id("test-cube", wallet, control);
                    shown
                        .focus
                        .iter()
                        .find(|(focus, _)| *focus == id)
                        .unwrap_or_else(|| panic!("no {} {}", wallet, control))
                        .1
                };
                let (spark, vault) = (at("Spark", "open"), at("Vault", "open"));
                if outer.0 == 1440 {
                    assert!(two_columns(width), "{}", width);
                    assert_eq!(spark.y, vault.y, "one row");
                    let column = (width - CARD_GAP) / 2.0;
                    assert!(
                        (vault.x - spark.x - (column + CARD_GAP)).abs() < 0.5,
                        "equal columns: {:?} {:?} in {}",
                        spark,
                        vault,
                        width
                    );
                    // Both cards keep Send and Receive on their BTC line at
                    // the fixture's balances, as their actions are the same
                    // width.
                    for wallet in ["Spark", "Vault"].iter().copied() {
                        let send = at(wallet, "send-btc");
                        let card_left = at(wallet, "open").x - 20.0;
                        let btc = shown
                            .text
                            .iter()
                            .find(|(value, bounds)| {
                                value == "BTC"
                                    && bounds.x >= card_left
                                    && bounds.x < card_left + column
                            })
                            .unwrap_or_else(|| panic!("no BTC label in {}", wallet))
                            .1;
                        assert!(
                            (send.center().y - btc.center().y).abs() < 2.0,
                            "{} ({}): Send {:?} off the BTC line {:?}",
                            wallet,
                            appearance,
                            send,
                            btc
                        );
                    }
                    // A half-width card keeps its row's actions inside it.
                    let receive = at("Spark", "receive-btc");
                    let card_right = spark.x - 20.0 + column;
                    assert!(
                        receive.x + receive.width <= card_right - 20.0 + 0.5,
                        "{:?} past {}",
                        receive,
                        card_right
                    );
                } else {
                    assert!(!two_columns(width), "{}", width);
                    assert_eq!(spark.x, vault.x, "stacked");
                    assert!(vault.y > spark.y);
                }
            }
        }
    }

    /// At the narrowest width that still sets two cards side by side (each
    /// 472px), a wallet's bitcoin row with the largest balance and its fiat
    /// value keeps Send and Receive whole and inside its card, clear of the
    /// amounts, in both themes.
    #[test]
    fn wallet_card_rows_fit_at_the_two_column_breakpoint() {
        use coincube_ui::component::tenshu::{cards::two_columns, CARD_GAP, CARD_MIN_WIDTH};
        let fixture = ViewFixture::new();
        let width = 2.0 * CARD_MIN_WIDTH + CARD_GAP;
        assert!(two_columns(width));
        let column = (width - CARD_GAP) / 2.0;
        // Every sat there will ever be, in each wallet.
        let most = Amount::from_sat(2_099_999_997_690_000);
        for (appearance, theme) in [
            ("dark", theme::Theme::dark()),
            ("light", theme::Theme::light()),
        ] {
            let mut config = fixture.config(0, None, None);
            config.has_liquid = false;
            config.spark_balance = most;
            config.spark_btc_balance = most;
            config.vault_balance = most;
            let shown = capture::inspect_wide_content(
                "overview-cards-breakpoint",
                overview(config),
                (1440, 900),
                appearance,
                theme,
                width as u32,
            );
            let at = |wallet: &str, control: &str| {
                let id = landing_control_id("test-cube", wallet, control);
                shown
                    .focus
                    .iter()
                    .find(|(focus, _)| *focus == id)
                    .unwrap_or_else(|| panic!("no {} {}", wallet, control))
                    .1
            };
            for (index, wallet) in ["Spark", "Vault"].iter().copied().enumerate() {
                let left = index as f32 * (column + CARD_GAP);
                let (send, receive) = (at(wallet, "send-btc"), at(wallet, "receive-btc"));
                assert!(
                    send.width >= 89.5 && receive.width >= 89.5,
                    "{}: {:?} {:?}",
                    wallet,
                    send,
                    receive
                );
                assert!(
                    send.x >= left && receive.x + receive.width <= left + column + 0.5,
                    "{} ({}): {:?} {:?} outside {}..{}",
                    wallet,
                    appearance,
                    send,
                    receive,
                    left,
                    left + column
                );
                // No text in the card overlaps the two actions' row band
                // horizontally unless it sits beside them, left of Send.
                let band = send.y..send.y + send.height;
                for (value, bounds) in &shown.text {
                    let in_card = bounds.x >= left && bounds.x < left + column;
                    let in_band = band.contains(&(bounds.y + bounds.height / 2.0));
                    if in_card && in_band && value != "Send" && value != "Receive" {
                        assert!(
                            bounds.x + bounds.width <= send.x + 0.5,
                            "{} ({}): {:?} {:?} runs under Send {:?}",
                            wallet,
                            appearance,
                            value,
                            bounds,
                            send
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn overview_context_alias_and_section_controls_render() {
        use crate::app::view::shared::landing::capture;
        let fixture = ViewFixture::new();
        for (appearance, theme) in [
            ("dark", theme::Theme::dark()),
            ("light", theme::Theme::light()),
        ] {
            for outer in [(1024, 768), (1440, 900)] {
                let shown = capture::inspect(
                    "overview-context",
                    overview(fixture.config(0, None, None)),
                    outer,
                    appearance,
                    theme,
                );
                // The Cube line moved to the page head; the body must not
                // repeat it.
                assert!(!shown.contains("Everyday · Bitcoin"));
                assert!(shown.contains("Vault · Family savings"));
                for section in ["Total Balance", "Wallets"] {
                    assert!(shown.has_focus(landing_control_id(
                        "test-cube",
                        section,
                        "balance-privacy"
                    )));
                }

                for (local_total, local_wallets) in [(false, false), (true, false), (false, true)] {
                    let mut global_hidden = fixture.config(0, None, None);
                    global_hidden.balance_masked = true;
                    global_hidden.overview_total_hidden = local_total;
                    global_hidden.overview_wallets_hidden = local_wallets;
                    let hidden = capture::inspect(
                        "overview-global-hide",
                        overview(global_hidden),
                        outer,
                        appearance,
                        theme,
                    );
                    assert!(!hidden.contains("250,000"));
                    assert!(!hidden.contains("31,000"));
                    for section in ["Total Balance", "Wallets"] {
                        assert!(!hidden.has_focus(landing_control_id(
                            "test-cube",
                            section,
                            "balance-privacy"
                        )));
                    }
                }
            }
        }
    }

    /// Activity as the Overview reads it in the `P-Overview` captures: Spark
    /// and Vault rows across today, yesterday and an older day.
    fn overview_activity() -> crate::app::state::activity::Activity {
        use crate::app::state::State;
        use chrono::Local;
        use coincube_core::miniscript::bitcoin::{
            absolute, hashes::Hash, transaction, Network, ScriptBuf, Transaction, TxOut,
            WScriptHash,
        };
        let at = |days_ago: i64, hour: u32, minute: u32| {
            (Local::now().date_naive() - chrono::Duration::days(days_ago))
                .and_hms_opt(hour, minute, 0)
                .unwrap()
                .and_local_timezone(Local)
                .earliest()
                .unwrap()
                .timestamp() as u64
        };
        let spark = |id: &str, time: u64, sats: i64, direction: &str, description: &str| {
            coincube_spark_protocol::PaymentSummary {
                id: id.into(),
                amount_sat: sats,
                fees_sat: 0,
                token_amount: None,
                token_decimals: None,
                token_ticker: None,
                timestamp: time,
                status: "completed".into(),
                direction: direction.into(),
                method: "lightning".into(),
                description: Some(description.into()),
                details: Default::default(),
            }
        };
        let received = |time: u64, sats: u64, n: u8| {
            let tx = Transaction {
                version: transaction::Version::TWO,
                lock_time: absolute::LockTime::from_consensus(u32::from(n)),
                input: Vec::new(),
                output: vec![TxOut {
                    value: Amount::from_sat(sats),
                    script_pubkey: ScriptBuf::new_p2wsh(&WScriptHash::all_zeros()),
                }],
            };
            crate::daemon::model::HistoryTransaction::new(
                tx,
                Some(900_000),
                Some(time as u32),
                Vec::new(),
                vec![0],
                Network::Bitcoin,
            )
        };
        let cache = crate::app::cache::Cache::default();
        let mut activity = crate::app::state::activity::Activity::new(None, None);
        drop(activity.update(
            None,
            &cache,
            crate::app::message::Message::ActivitySparkLoaded(
                0,
                Ok((
                    vec![
                        spark("p1", at(0, 14, 32), 25_053, "receive", "Received"),
                        spark("p2", at(0, 9, 40), -4_200, "send", "Coffee"),
                        spark("p3", at(1, 11, 5), 12_784, "receive", "Lunch split"),
                    ],
                    false,
                )),
            ),
        ));
        drop(activity.update(
            None,
            &cache,
            crate::app::message::Message::ActivityVaultLoaded(
                0,
                Ok((
                    vec![
                        received(at(1, 21, 15), 40_000, 1),
                        received(at(12, 8, 0), 125_000, 2),
                    ],
                    false,
                )),
            ),
        ));
        activity
    }

    /// `P-Overview` under its pinned head, at the top and scrolled: the
    /// Transfer head action, the wallet cards side by side at 1440×900 and
    /// stacked at 1024×768, then "Recent activity" (Tenshu v175 L3 and W1).
    /// Liquid is hidden in v175. Opt-in: `TOP_BAR_CAPTURE_DIR=<dir> cargo
    /// test -p coincube-gui --lib page_head_capture -- --ignored`.
    #[test]
    #[ignore = "writes PNG frames; needs a GPU adapter"]
    fn page_head_capture_overview() {
        use crate::app::state::preferences::BalancePrivacy;
        use crate::app::view::activity::{recent_activity, RecentScope};
        use crate::app::view::nav::shell::capture_tests;
        let Some(directory) = capture_tests::directory() else {
            return;
        };
        let mut renderer = capture_tests::renderer();
        let fixture = ViewFixture::new();
        let activity = overview_activity();
        let menu = Menu::Cube(crate::app::menu::CubeSubMenu::Overview);
        for (appearance, palette) in capture_tests::themes() {
            let light = appearance == "light";
            let cache = capture_tests::cache(light, BalancePrivacy::Shown);
            capture_tests::save_page_head_frames(
                &mut renderer,
                &directory,
                "overview",
                appearance,
                &palette,
                || {
                    let mut config = fixture.config(0, None, None);
                    config.light_theme = light;
                    config.has_liquid = false;
                    config.pending_spark_incoming = None;
                    config.pending_vault_incoming = None;
                    let transfer = transfer_head_action(&config);
                    overview_page(
                        &menu,
                        &cache,
                        transfer,
                        overview(config),
                        Some(recent_activity(&activity, &cache, RecentScope::Cube, None)),
                    )
                },
            );
        }
    }

    #[test]
    fn global_home_overview_builds_loaded_masked_loading_and_absent_wallet_states() {
        let fixture = ViewFixture::new();

        let _ = overview(fixture.config(0, None, None));

        let mut masked = fixture.config(0, None, None);
        masked.balance_masked = true;
        masked.display_mode = DisplayMode::BitcoinNative;
        let _ = overview(masked);

        let mut loading = fixture.config(0, None, None);
        loading.total_balance_loading = true;
        loading.spark_balance_loaded = false;
        loading.liquid_balance_loaded = false;
        loading.usdt_balance_loaded = false;
        loading.vault_loaded = false;
        loading.usdt_balance_error = true;
        let _ = overview(loading);

        // A failed fetch: loaded (so the aggregate settles) but flagged, so the
        // row says "Balance unavailable" rather than rendering the zero it
        // never managed to overwrite.
        let mut liquid_failed = fixture.config(0, None, None);
        liquid_failed.liquid_balance_loaded = true;
        liquid_failed.liquid_balance_error = true;
        let _ = overview(liquid_failed);

        let mut spark_only = fixture.config(0, None, None);
        spark_only.has_liquid = false;
        spark_only.has_vault = false;
        spark_only.has_spark = true;
        spark_only.pending_spark_incoming = Some(PendingTransfer {
            amount: Amount::from_sat(11_000),
            stage: TransferStage::Completed,
        });
        let _ = overview(spark_only);
    }

    #[test]
    fn global_home_transfer_form_builds_every_direction_and_available_pair() {
        let fixture = ViewFixture::new();

        for direction in directions() {
            let _ = transfer_dialog_for(fixture.config(1, Some(direction), None));
        }

        let mut no_liquid = fixture.config(1, Some(TransferDirection::VaultToSpark), None);
        no_liquid.has_liquid = false;
        let _ = transfer_dialog_for(no_liquid);
    }

    #[test]
    fn global_home_confirm_step_builds_address_fee_and_sending_variants() {
        let fixture = ViewFixture::new();

        let mut liquid_to_vault = fixture.config(2, Some(TransferDirection::LiquidToVault), None);
        let liquid_prepare = PreparePayOnchainResponse {
            receiver_amount_sat: 49_000,
            claim_fees_sat: 100,
            total_fees_sat: 1_000,
        };
        liquid_to_vault.prepare_onchain_send_response = Some(&liquid_prepare);
        let _ = transfer_dialog_for(liquid_to_vault);

        let mut vault_to_spark = fixture.config(2, Some(TransferDirection::VaultToSpark), None);
        vault_to_spark.address_expanded = true;
        vault_to_spark.spend_tx_fees = Some(Amount::from_sat(750));
        vault_to_spark.transfer_feerate_loading = Some(FeeratePreset::Fast);
        let _ = transfer_dialog_for(vault_to_spark);

        let mut vault_to_liquid_signed =
            fixture.config(2, Some(TransferDirection::VaultToLiquid), None);
        vault_to_liquid_signed.is_tx_signed = true;
        vault_to_liquid_signed.is_sending = true;
        vault_to_liquid_signed.spend_tx_fees = Some(Amount::from_sat(900));
        let _ = transfer_dialog_for(vault_to_liquid_signed);

        let mut spark_to_liquid = fixture.config(2, Some(TransferDirection::SparkToLiquid), None);
        spark_to_liquid.spark_send_fee_sat = Some(222);
        spark_to_liquid.receive_address = None;
        let _ = transfer_dialog_for(spark_to_liquid);

        let mut invalid_vault_send =
            fixture.config(2, Some(TransferDirection::VaultToLiquid), None);
        invalid_vault_send.entered_amount = &fixture.invalid_amount;
        invalid_vault_send.transfer_feerate = &fixture.invalid_feerate;
        let _ = transfer_dialog_for(invalid_vault_send);
    }

    #[test]
    fn global_home_success_and_fallback_steps_build() {
        let fixture = ViewFixture::new();

        for direction in directions() {
            let _ = transfer_dialog_for(fixture.config(3, Some(direction), None));
        }

        assert!(
            global_home_view(fixture.config(99, Some(TransferDirection::LiquidToVault), None))
                .1
                .is_none()
        );
    }

    fn inspect_dialog(scenario: &str, config: GlobalViewConfig<'_>) -> capture::Inspection {
        capture::inspect(
            scenario,
            transfer_dialog_for(config),
            (1440, 900),
            "dark",
            theme::Theme::dark(),
        )
    }

    /// The Transfer context `global_home_view` builds, for rendering a
    /// route tile's menu on its own (the capture doesn't draw overlays).
    fn menu_ctx<'a>(config: &GlobalViewConfig<'a>) -> TransferCtx<'a> {
        TransferCtx {
            cube_id: config.cube_id,
            cube_name: config.cube_name,
            chain: config.chain,
            direction: config.transfer_direction.expect("a direction"),
            has_vault: config.has_vault,
            has_spark: config.has_spark,
            has_liquid: config.has_liquid,
            liquid_balance: config.liquid_balance,
            spark_balance: config.spark_balance,
            vault_balance: config.vault_balance,
            fiat_converter: config.fiat_converter,
            unit: config.transfer_unit,
            max: config.transfer_max,
            balance_masked: config.balance_masked,
            other: config.other_cube,
            menu: config.wallet_picker,
            origin: config.transfer_origin,
        }
    }

    fn inspect_menu(scenario: &str, menu: Element<'_, Message>) -> capture::Inspection {
        capture::inspect(scenario, menu, (1440, 900), "dark", theme::Theme::dark())
    }

    fn cube_choice(name: &str, chain: ChainId, refusal: Option<&'static str>) -> CubeChoice {
        CubeChoice {
            cube_id: format!("{name}-id"),
            name: name.to_string(),
            chain,
            has_vault: true,
            has_spark: false,
            passkey: false,
            vault_wallet_id: Some(crate::app::settings::WalletId::new(name.into(), Some(1))),
            refusal,
        }
    }

    /// Every route has the same steps: Details, Review, Sent. Another Cube
    /// is chosen and unlocked on the Details card.
    #[test]
    fn transfer_steps_follow_the_route() {
        let fixture = ViewFixture::new();
        let choices = [cube_choice("Archive", ChainId::Bitcoin, None)];
        for direction in [
            TransferDirection::SparkToVault,
            TransferDirection::VaultToOtherCube,
        ] {
            for (step, page) in [
                (1, TransferPage::Wallets),
                (1, TransferPage::Unlock),
                (1, TransferPage::Amount),
                (2, TransferPage::Amount),
                (3, TransferPage::Amount),
            ] {
                let mut config = fixture.config(step, Some(direction), None);
                config.transfer_page = page;
                config.other_cube.choices = &choices;
                config.other_cube.selected = Some("Archive-id");
                let snapshot = inspect_dialog("transfer-steps", config);
                for label in TRANSFER_STEPS {
                    assert!(
                        snapshot.contains(label),
                        "{:?} {:?} {}: {}",
                        direction,
                        page,
                        step,
                        label
                    );
                }
                assert!(
                    !snapshot.contains("Other Cube"),
                    "{:?} {:?} {}",
                    direction,
                    page,
                    step
                );
            }
        }
    }

    #[test]
    fn transfer_pages_match_the_prototype() {
        let fixture = ViewFixture::new();
        let mut amount = fixture.config(1, Some(TransferDirection::SparkToVault), None);
        amount.transfer_page = TransferPage::Amount;
        let amount = inspect_dialog("transfer-details", amount);
        for label in [
            "Transfer",
            "Everyday · Bitcoin mainnet",
            "Transfer details",
            "From",
            "Available 250,000 SATS",
            "in Everyday",
            "To",
            "Balance 375,000 SATS",
            "SATS",
            "Minimum",
            "They receive about",
            "Bitcoin on-chain",
            "Shown when you prepare",
            "Prepare",
            "Available in Everyday",
            "This transfer",
            "Quoted at Prepare",
        ] {
            assert!(amount.contains(label), "details: {}", label);
        }
        // The old page buttons are gone: Back to Overview is the head's.
        assert!(!amount.contains("Continue"));
        assert!(!amount.text.iter().any(|(value, _)| value == "Back"));
        assert!(amount.contains("Back to Overview"));
        assert!(
            !amount.contains("BTC"),
            "the unit follows the wallet setting"
        );

        let review = inspect_dialog(
            "transfer-review",
            fixture.config(2, Some(TransferDirection::SparkToVault), None),
        );
        for label in [
            "You transfer",
            "They receive",
            "Edit",
            "This transfer",
            "Bitcoin SHA-256 · Mainnet",
            "Sending from",
            "Spark · Everyday",
            "Receiving wallet",
            "Route",
            "Bitcoin on-chain",
            "Fees",
            "Confirm transfer",
            "Address details",
        ] {
            assert!(review.contains(label), "review: {}", label);
        }
        assert!(review.contains(MAINNET_ADDR), "whole address");
        assert!(!review.contains("Show QR code"), "details stay collapsed");
        let mut expanded = fixture.config(2, Some(TransferDirection::SparkToVault), None);
        expanded.address_expanded = true;
        let expanded = inspect_dialog("transfer-review-expanded", expanded);
        assert!(expanded.contains("Show QR code") && expanded.contains("Vault receive"));

        let done = inspect_dialog(
            "transfer-done",
            fixture.config(3, Some(TransferDirection::SparkToVault), None),
        );
        for label in [
            "Transfer sent",
            "Pending",
            "you can leave this page",
            "See it in Activity",
            "Back to Overview",
            "New transfer",
        ] {
            assert!(done.contains(label), "done: {}", label);
        }
    }

    #[test]
    fn other_cube_pages_list_unlock_and_relock() {
        let fixture = ViewFixture::new();
        let choices = [
            cube_choice("Archive", ChainId::Bitcoin, None),
            cube_choice(
                "Lab",
                ChainId::Bitcoin,
                Some("No Vault to receive this transfer"),
            ),
        ];
        let other = |page, step| {
            let mut config = fixture.config(step, Some(TransferDirection::VaultToOtherCube), None);
            config.transfer_page = page;
            config.other_cube.choices = &choices;
            config.other_cube.selected = Some("Archive-id");
            config
        };

        let mut config = other(TransferPage::Amount, 1);
        config.wallet_picker = Some(PickerSide::To);
        let menu = inspect_menu("transfer-to-menu", to_menu(&menu_ctx(&config)));
        for label in [
            "THIS CUBE · EVERYDAY",
            "Spark",
            "OTHER CUBES",
            "Vault in Archive",
            "Locked · unlocks for this transfer",
            "Lab",
            "No Vault to receive this transfer",
            "Locked Cubes show no balances.",
        ] {
            assert!(menu.contains(label), "to menu: {}", label);
        }

        let unlock = inspect_dialog("transfer-unlock", other(TransferPage::Unlock, 1));
        for label in [
            "Enter Archive's PIN to transfer into it",
            "Not Everyday's PIN. Archive locks again when this transfer ends.",
            "Unlock",
            "Vault",
            "in Archive",
            "Locked",
            "Unlock Archive to enter an amount.",
        ] {
            assert!(unlock.contains(label), "unlock: {}", label);
        }
        assert!(
            !unlock.contains("Unlock with passkey"),
            "a PIN Cube shows its PIN only"
        );
        let mut passkey_choices = choices.clone();
        passkey_choices[0].passkey = true;
        let mut passkey = other(TransferPage::Unlock, 1);
        passkey.other_cube.choices = &passkey_choices;
        let passkey = inspect_dialog("transfer-unlock-passkey", passkey);
        assert!(passkey.contains("Unlock with passkey"));
        assert!(
            !passkey.contains("PIN"),
            "a passkey Cube shows its passkey only"
        );

        // A pick whose unlock couldn't start offers no PIN or passkey control.
        let mut refused = other(TransferPage::OtherCube, 1);
        refused.other_cube.error = Some("This Cube changed. Choose it again.");
        let refused = inspect_dialog("transfer-unlock-refused", refused);
        assert!(refused.contains("This Cube changed. Choose it again."));
        assert!(refused.contains("Choose it again in To"));
        assert!(
            !refused.contains("Unlock with passkey") && !refused.contains("Enter Archive's PIN")
        );

        let mut checking = other(TransferPage::Unlock, 1);
        checking.other_cube.phase = UnlockPhase::Checks(2);
        let checking = inspect_dialog("transfer-unlock-checks", checking);
        for label in [
            "PIN accepted",
            "Archive's Vault checked",
            "Getting a receive address…",
            "In progress",
        ] {
            assert!(checking.contains(label), "checks: {}", label);
        }

        let mut amount = other(TransferPage::Amount, 1);
        amount.other_cube.unlocked = true;
        let amount = inspect_dialog("transfer-other-amount", amount);
        assert!(amount.contains("Unlocked for this transfer"));
        assert!(amount.contains("Vault in Archive after"));

        let mut review = other(TransferPage::Amount, 2);
        review.other_cube.unlocked = true;
        let review = inspect_dialog("transfer-other-review", review);
        assert!(review.contains("Vault · Archive · Bitcoin mainnet"));
        assert!(review.contains("Archive locks again once sent, or if you leave."));
        assert!(review.contains(MAINNET_ADDR));
        // Labels and QR belong to this Cube's own addresses only.
        assert!(!review.contains("Address details"));

        let mut done = other(TransferPage::Amount, 3);
        done.other_cube.locked_again = true;
        let done = inspect_dialog("transfer-other-done", done);
        assert!(done.contains("Archive is locked again."));
        assert!(done.contains("Vault · Archive"));
    }

    /// Draws `content` with its overlays (an open route menu) on the
    /// software renderer and saves it under `directory`.
    fn save_with_overlays(
        directory: &str,
        name: &str,
        mut element: Element<'_, Message>,
        (width, height): (u32, u32),
        theme: &theme::Theme,
    ) {
        use iced::advanced::{
            layout::{self, Layout},
            overlay,
            renderer::Headless,
            widget::Tree,
        };
        use iced::{mouse, Rectangle, Size, Vector};
        for bytes in coincube_ui::font::load() {
            iced::advanced::graphics::text::font_system()
                .write()
                .unwrap()
                .load_font(bytes);
        }
        let size = Size::new(width as f32, height as f32);
        let mut renderer = iced::futures::executor::block_on(<iced::Renderer as Headless>::new(
            coincube_ui::font::REGULAR,
            iced::Pixels(16.0),
            Some("tiny-skia"),
        ))
        .expect("software renderer");
        let mut tree = Tree::new(element.as_widget());
        let node = element.as_widget_mut().layout(
            &mut tree,
            &renderer,
            &layout::Limits::new(Size::ZERO, size),
        );
        let viewport = Rectangle::with_size(size);
        element.as_widget_mut().update(
            &mut tree,
            &iced::Event::Window(iced::window::Event::RedrawRequested(
                std::time::Instant::now(),
            )),
            Layout::new(&node),
            mouse::Cursor::Unavailable,
            &renderer,
            &mut iced::advanced::clipboard::Null,
            &mut iced::advanced::Shell::new(&mut Vec::new()),
            &viewport,
        );
        let style = iced::advanced::renderer::Style {
            text_color: theme.colors.text.primary,
        };
        element.as_widget().draw(
            &tree,
            &mut renderer,
            theme,
            &style,
            Layout::new(&node),
            mouse::Cursor::Unavailable,
            &viewport,
        );
        if let Some(mut overlay) = element
            .as_widget_mut()
            .overlay(
                &mut tree,
                Layout::new(&node),
                &renderer,
                &viewport,
                Vector::ZERO,
            )
            .map(overlay::Nested::new)
        {
            let node = overlay.layout(&renderer, size);
            overlay.draw(
                &mut renderer,
                theme,
                &style,
                Layout::new(&node),
                mouse::Cursor::Unavailable,
            );
        }
        let pixels = renderer.screenshot(
            size_u32(width, height),
            1.0,
            theme.colors.general.background,
        );
        std::fs::create_dir_all(directory).unwrap();
        image::save_buffer(
            std::path::Path::new(directory).join(format!("{name}.png")),
            &pixels,
            width,
            height,
            image::ColorType::Rgba8,
        )
        .unwrap();
    }

    fn size_u32(width: u32, height: u32) -> iced::Size<u32> {
        iced::Size::new(width, height)
    }

    /// Review evidence: set `TRANSFER_CAPTURE_DIR` to save the Details card
    /// (each menu open, another Cube's PIN or passkey unlock), Review and
    /// Sent, dark and light.
    #[test]
    #[ignore = "writes PNGs; set TRANSFER_CAPTURE_DIR"]
    fn transfer_route_cards_capture() {
        let Ok(directory) = std::env::var("TRANSFER_CAPTURE_DIR") else {
            return;
        };
        let fixture = ViewFixture::new();
        let mut archive = cube_choice("Archive", ChainId::Bitcoin, None);
        archive.has_spark = true;
        let mut passkey = archive.clone();
        passkey.passkey = true;
        let choices = [
            archive,
            cube_choice(
                "Lab",
                ChainId::Bitcoin,
                Some("No Vault to receive this transfer"),
            ),
        ];
        let passkey_choices = [passkey];
        let pin = crate::pin_input::PinInput::new();
        for (appearance, theme) in [
            ("dark", theme::Theme::dark()),
            ("light", theme::Theme::light()),
        ] {
            let shots: Vec<(&str, GlobalViewConfig<'_>)> = vec![
                ("1-details", {
                    let mut c = fixture.config(1, Some(TransferDirection::SparkToVault), None);
                    c.transfer_page = TransferPage::Amount;
                    c
                }),
                ("2-from-menu", {
                    let mut c = fixture.config(
                        1,
                        Some(TransferDirection::SparkToVault),
                        Some(PickerSide::From),
                    );
                    c.transfer_page = TransferPage::Amount;
                    c
                }),
                ("3-to-menu", {
                    let mut c = fixture.config(
                        1,
                        Some(TransferDirection::SparkToVault),
                        Some(PickerSide::To),
                    );
                    c.transfer_page = TransferPage::Amount;
                    c.other_cube.choices = &choices;
                    c
                }),
                ("4-unlock-pin", {
                    let mut c = fixture.config(1, Some(TransferDirection::SparkToOtherSpark), None);
                    c.transfer_page = TransferPage::Unlock;
                    c.other_cube.choices = &choices;
                    c.other_cube.selected = Some("Archive-id");
                    c.other_cube.pin = Some(&pin);
                    c
                }),
                ("5-unlock-passkey", {
                    let mut c = fixture.config(1, Some(TransferDirection::SparkToOtherSpark), None);
                    c.transfer_page = TransferPage::Unlock;
                    c.other_cube.choices = &passkey_choices;
                    c.other_cube.selected = Some("Archive-id");
                    c
                }),
                ("6-unlocked", {
                    let mut c = fixture.config(1, Some(TransferDirection::SparkToOtherSpark), None);
                    c.transfer_page = TransferPage::Amount;
                    c.other_cube.choices = &choices;
                    c.other_cube.selected = Some("Archive-id");
                    c.other_cube.unlocked = true;
                    c
                }),
                (
                    "7-review",
                    fixture.config(2, Some(TransferDirection::SparkToVault), None),
                ),
                ("8-review-spark-to-spark", {
                    let mut c = fixture.config(2, Some(TransferDirection::SparkToOtherSpark), None);
                    c.other_cube.choices = &choices;
                    c.other_cube.selected = Some("Archive-id");
                    c.other_cube.unlocked = true;
                    c.other_cube.spark_address = Some("sp1test-address");
                    c.spark_send_fee_sat = Some(0);
                    c
                }),
                (
                    "9-sent",
                    fixture.config(3, Some(TransferDirection::SparkToVault), None),
                ),
            ];
            for (name, mut config) in shots {
                config.light_theme = appearance == "light";
                save_with_overlays(
                    &directory,
                    &format!("transfer-{name}-{appearance}"),
                    transfer_dialog_for(config),
                    (1440, 900),
                    &theme,
                );
            }
        }
    }

    #[test]
    fn spark_only_mainnet_cube_can_open_cross_cube_transfer() {
        let fixture = ViewFixture::new();
        let mut config = fixture.config(1, Some(TransferDirection::SparkToOtherCube), None);
        config.has_liquid = false;
        config.has_vault = false;
        assert!(transfer_head_action(&config).is_some());
        config.chain = ChainId::Regtest;
        assert!(transfer_head_action(&config).is_none());
        config.chain = ChainId::Bitcoin;

        let wallets = inspect_dialog("spark-only-transfer-wallets", config);
        assert!(wallets.contains("Another Cube"));
        assert!(wallets.contains("Choose another Cube in To."));
    }

    #[test]
    fn cross_cube_spark_review_shows_the_destination_and_payment_method() {
        let fixture = ViewFixture::new();
        let mut choice = cube_choice("Archive", ChainId::Bitcoin, None);
        choice.has_spark = true;
        let choices = [choice];
        for direction in [
            TransferDirection::SparkToOtherSpark,
            TransferDirection::VaultToOtherSpark,
        ] {
            let mut config = fixture.config(2, Some(direction), None);
            config.other_cube.choices = &choices;
            config.other_cube.selected = Some("Archive-id");
            config.other_cube.unlocked = true;
            let native = direction == TransferDirection::SparkToOtherSpark;
            if native {
                config.other_cube.spark_address = Some("sp1test-address");
                config.receive_address = None;
            }
            let review = inspect_dialog("cross-cube-spark-review", config);
            assert!(review.contains("Spark · Archive · Bitcoin mainnet"));
            if native {
                assert!(review.contains("sp1test-address"));
                assert!(review.contains("Confirm transfer"));
                assert!(!review.contains("Sign transaction"));
            } else {
                assert!(review.contains("Bitcoin on-chain"));
                assert!(review.contains("Sign transaction"));
            }
        }
    }
}

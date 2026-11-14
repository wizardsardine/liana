//! View renderer for [`crate::app::state::spark::overview::SparkOverview`].
//!
//! The Spark wallet page (Tenshu v175 W2, board `P-Spark`):
//! - "Total balance" with the Lightning address button beside it, which
//!   opens the share popover under it ([`lightning_share_panel`]), then one asset row
//!   per balance: Bitcoin, and US Dollar while the wallet holds USDB
//!   ([`spark_assets`]).
//! - "Recent activity", mounted by App from Activity.
//!
//! Claiming and changing the address happen in Settings › Spark › Lightning
//! address, as does the Stable Balance control (Balances & conversion).

use coincube_core::miniscript::bitcoin::Amount;
use coincube_ui::locale::copy as localized_copy;
use coincube_ui::localized_format;

use crate::app::wallets::DomainPaymentStatus;
use coincube_ui::component::tenshu::{
    asset_row, fit_row, ActionButton, ActionKind, AmountVisibility, BalanceValue, Figure,
};
use coincube_ui::{
    component::{amount::*, text::*},
    icon, theme,
    theme::tenshu::size,
    widget::{action::Action, floating::Floating, *},
};
use iced::{
    widget::{qr_code, Column, Container, QRCode, Row},
    Alignment, Length,
};

use crate::app::settings::display::DisplayMode;
use crate::app::state::spark::overview::SparkBalanceSnapshot;
use crate::app::view::shared::landing::{
    amount_cell, landing_balance_section, landing_button, landing_card_style, landing_control_id,
    AmountCell, AmountTier,
};
use crate::app::view::spark::settings::lightning_address::LN_ADDRESS_DOMAIN;
use crate::app::view::vault::fiat::FiatAmount;
use crate::app::view::wallet_header::{wallet_header, HeaderVariant, SyncState, WalletHeaderProps};
use crate::app::view::{FiatAmountConverter, SparkOverviewMessage};

/// High-level status of the Spark backend for the current cube.
#[derive(Debug, Clone)]
pub enum SparkStatus {
    /// No Spark signer configured for this cube (or bridge spawn failed).
    Unavailable,
    /// First `get_info` is still in flight.
    Loading,
    /// Bridge returned a balance snapshot.
    Connected(SparkBalanceSnapshot),
    /// Bridge returned an error response.
    Error(String),
}

/// Current bridge state for the contextual action. A routine refresh is
/// distinct from a failed bridge even while both keep the action disabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SparkLandingAvailability {
    Ready,
    Refreshing,
    Unavailable,
}

/// Which underlying Spark payment rail fulfilled a transaction. Drives
/// the asset icon on the overview row — Spark-native and Lightning
/// payments share the same wallet balance but should look different at
/// a glance.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum SparkPaymentMethod {
    Lightning,
    OnChainBitcoin,
    Spark,
    /// Stable Balance conversion leg (BTC ↔ USDB). Surfaces in the
    /// transaction list whenever the SDK records the receive side of
    /// a conversion as a top-level `method=token` payment. Rendered
    /// with the "Stable" branding rather than as a bitcoin transfer
    /// — the amount is in token base units, not sats.
    StableBalance,
}

/// Row data the overview renderer needs for each recent payment.
/// Mirrors `view::liquid::RecentTransaction` plus a `token_display`
/// override that the Stable Balance USDB rows use the same way Liquid
/// uses `usdt_display` — when set, the row shows that string in place
/// of the BTC amount and skips the BTC pending-sums.
///
/// Carries the Spark payment `id` and raw `timestamp` so the detail
/// view can render a full date and a copy-to-clipboard payment ID
/// without needing to hold onto the originating `PaymentSummary`.
#[derive(Debug, Clone)]
pub struct SparkRecentTransaction {
    pub id: String,
    pub description: String,
    pub time_ago: String,
    pub timestamp: u64,
    pub amount: Amount,
    pub fees_sat: Amount,
    pub fiat_amount: Option<FiatAmount>,
    pub is_incoming: bool,
    pub status: DomainPaymentStatus,
    pub method: SparkPaymentMethod,
    /// When set, render this string instead of the BTC `amount` (e.g.
    /// "1.58 USDB"). Token-method payments populate this; bitcoin
    /// rows leave it `None`.
    pub token_display: Option<String>,
}

impl SparkRecentTransaction {
    /// BTC headline amount: incoming net credit, or outgoing debit including fees.
    /// Token rows render `token_display` instead and are excluded from BTC totals.
    pub fn display_amount(&self) -> Amount {
        if self.is_incoming {
            self.amount
        } else {
            self.amount + self.fees_sat
        }
    }
}

/// View wrapper for the Spark Overview panel.
pub struct SparkOverviewView<'a> {
    pub status: SparkStatus,
    pub recent_transactions: &'a [SparkRecentTransaction],
    pub cube_id: &'a str,
    /// An error can coexist with a retained last-success snapshot.
    pub last_error: Option<&'a str>,
    pub fiat_converter: Option<FiatAmountConverter>,
    pub bitcoin_unit: BitcoinDisplayUnit,
    pub show_direction_badges: bool,
    /// Reference BTC/USD price used to fold USDB into the unified
    /// portfolio total at the current rate. `None` skips the fold —
    /// matches the Liquid panel's USDt behaviour.
    pub btc_usd_price: Option<f64>,
    pub display_mode: DisplayMode,
    pub mask_amounts: bool,
    pub lightning_address: SparkLightningAddress,
    /// The open share popover: its address and that address's QR code.
    pub lightning_share: Option<(&'a str, Option<&'a qr_code::Data>)>,
}

/// What the Spark page knows about the Cube's Lightning address. It drives
/// the button beside "Total balance".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SparkLightningAddress {
    /// Not known yet (the Cube's registration is still in flight): no button,
    /// rather than a claim prompt that flips to the address a moment later.
    Unknown,
    /// None claimed, or signed out: the button leads to the claim form.
    Unclaimed,
    /// The full address, domain included.
    Ready(String),
}

/// The full Lightning address for a stored value that may be a bare username.
pub fn full_lightning_address(raw: &str) -> String {
    if raw.contains('@') {
        raw.to_owned()
    } else {
        format!("{raw}{LN_ADDRESS_DOMAIN}")
    }
}

/// The button beside "Total balance": the address and an invitation to share
/// it, or to claim one. Never masked; an address is not an amount. `open`
/// keeps its accent border while the share popover hangs from it.
fn lightning_address_button<'a>(
    cube_id: &str,
    address: &SparkLightningAddress,
    open: bool,
) -> Option<Element<'a, SparkOverviewMessage>> {
    let (control, title, subtitle, trailing, press) = match address {
        SparkLightningAddress::Unknown => return None,
        SparkLightningAddress::Unclaimed => (
            "ln-claim",
            "Claim your Lightning address".to_owned(),
            "Get paid to a readable address",
            None,
            SparkOverviewMessage::ClaimLightningAddress,
        ),
        SparkLightningAddress::Ready(address) => (
            "ln-share",
            crate::utils::truncate_middle(address, 18, 14),
            "Your Lightning address · share it to get paid",
            Some("Share"),
            SparkOverviewMessage::ShareLightningAddress(address.clone()),
        ),
    };
    let bolt = Container::new(
        icon::lightning_icon()
            .size(20)
            .color(theme::tenshu::ON_ACCENT),
    )
    .width(Length::Fixed(40.0))
    .height(Length::Fixed(40.0))
    .align_x(Alignment::Center)
    .align_y(Alignment::Center)
    .style(|theme: &theme::Theme| iced::widget::container::Style {
        background: Some(iced::Background::Color(theme.colors.shell.accent)),
        border: iced::Border {
            radius: 10.0.into(),
            ..Default::default()
        },
        ..Default::default()
    });
    let mut content = Row::new()
        .spacing(12)
        .align_y(Alignment::Center)
        .push(bolt)
        .push(
            Column::new()
                .spacing(2)
                .push(p1_bold(title).wrapping(iced::widget::text::Wrapping::None))
                .push(
                    text(subtitle)
                        .size(P2_SIZE)
                        .style(theme::text::secondary)
                        .wrapping(iced::widget::text::Wrapping::None),
                ),
        );
    if let Some(label) = trailing {
        content = content.push(iced::widget::rule::vertical(1)).push(
            Row::new()
                .spacing(6)
                .align_y(Alignment::Center)
                .push(icon::share_icon().size(size::ICON_SMALL).style(accent_text))
                .push(
                    p2_regular(label)
                        .font(coincube_ui::font::SEMIBOLD)
                        .style(accent_text),
                ),
        );
    }
    Some(
        Action::new(
            landing_control_id(cube_id, "Spark", control),
            Container::new(content.height(Length::Fixed(40.0))).padding([10, 12]),
            SparkOverviewMessage::FocusAction,
        )
        .style(move |theme, status| lightning_address_button_style(theme, status, open))
        .focus_radius(12.0)
        .on_press(press)
        .into(),
    )
}

fn accent_text(theme: &theme::Theme) -> iced::widget::text::Style {
    iced::widget::text::Style {
        color: Some(theme.colors.shell.accent_ink),
    }
}

/// The address button's surface: a panel card whose border takes the accent
/// under the pointer and while its popover is open.
fn lightning_address_button_style(
    theme: &theme::Theme,
    status: iced::widget::button::Status,
    open: bool,
) -> iced::widget::button::Style {
    use iced::widget::button::Status;
    let shell = &theme.colors.shell;
    iced::widget::button::Style {
        background: Some(iced::Background::Color(shell.panel)),
        text_color: shell.ink,
        border: iced::Border {
            color: if open || matches!(status, Status::Hovered | Status::Pressed) {
                shell.accent_ink
            } else {
                shell.line
            },
            width: 1.0,
            radius: 12.0.into(),
        },
        ..Default::default()
    }
}

/// The share popover the address button opens, anchored under it: the
/// address as a QR code (encoded exactly as Spark › Receive encodes it, the
/// bare address), copy and save actions and the way to a different name.
pub fn lightning_share_panel<'a>(
    cube_id: &str,
    address: &'a str,
    qr: Option<&'a qr_code::Data>,
) -> Element<'a, SparkOverviewMessage> {
    let has_qr = qr.is_some();
    let qr: Element<'a, SparkOverviewMessage> = match qr {
        Some(qr) => Container::new(QRCode::<theme::Theme>::new(qr).total_size(184))
            .width(Length::Fill)
            .align_x(Alignment::Center)
            .into(),
        None => p2_regular(localized_copy("QR unavailable · copy the address instead."))
            .style(theme::text::secondary)
            .into(),
    };
    let change = ActionButton::new(
        landing_control_id(cube_id, "Spark", "ln-change"),
        localized_copy("Change address"),
        ActionKind::Quiet,
        SparkOverviewMessage::FocusAction,
    )
    .on_press(SparkOverviewMessage::ChangeLightningAddress);
    let panel = Column::new()
        .spacing(16)
        .push(
            Column::new()
                .spacing(4)
                .push(h4_bold(localized_copy("Get paid over Lightning")))
                .push(
                    text(localized_copy(
                        "Anyone with a Lightning wallet can send you bitcoin at this \
                         address, even while Tenshu is closed.",
                    ))
                    .size(P2_SIZE)
                    .style(theme::text::secondary),
                ),
        )
        .push(qr)
        .push(
            Container::new(text(address).size(18))
                .width(Length::Fill)
                .align_x(Alignment::Center),
        )
        .push({
            let copy = ActionButton::new(
                landing_control_id(cube_id, "Spark", "ln-copy"),
                localized_copy("Copy address"),
                ActionKind::Primary,
                SparkOverviewMessage::FocusAction,
            )
            .icon(icon::copy_icon())
            .compact()
            .width(Length::Fill)
            .on_press(SparkOverviewMessage::CopyLightningAddress(
                address.to_owned(),
            ));
            let mut actions = Row::new().spacing(12).push(copy);
            // Saving needs the QR code; without one, copying is the only way.
            if has_qr {
                actions = actions.push(
                    ActionButton::new(
                        landing_control_id(cube_id, "Spark", "ln-save-qr"),
                        localized_copy("Save QR image"),
                        ActionKind::Secondary,
                        SparkOverviewMessage::FocusAction,
                    )
                    .icon(icon::import_icon())
                    .compact()
                    .width(Length::Fill)
                    .on_press(SparkOverviewMessage::SaveLightningQr(address.to_owned())),
                );
            }
            actions
        })
        .push(iced::widget::rule::horizontal(1))
        .push(
            Row::new()
                .align_y(Alignment::Center)
                .push(
                    text(localized_copy("Want a different name?"))
                        .size(P2_SIZE)
                        .style(theme::text::secondary)
                        .width(Length::Fill),
                )
                .push(change),
        );
    Container::new(panel)
        .padding(20)
        .width(Length::Fixed(380.0))
        .style(|theme: &theme::Theme| iced::widget::container::Style {
            border: iced::Border {
                radius: 12.0.into(),
                ..landing_card_style(theme).border
            },
            ..landing_card_style(theme)
        })
        .into()
}

/// The balances the Spark page lists, in order. Bitcoin always, unless the
/// wallet holds only USDB; US Dollar while it holds any.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SparkAsset {
    Bitcoin,
    UsDollar,
}

pub fn spark_assets(balance_sats: u64, usdb_units: u128) -> Vec<SparkAsset> {
    let bitcoin = (balance_sats > 0 || usdb_units == 0).then_some(SparkAsset::Bitcoin);
    let us_dollar = (usdb_units > 0).then_some(SparkAsset::UsDollar);
    bitcoin.into_iter().chain(us_dollar).collect()
}

impl<'a> SparkOverviewView<'a> {
    pub fn render(self) -> Element<'a, SparkOverviewMessage> {
        match &self.status {
            SparkStatus::Unavailable => Column::new()
                .spacing(20)
                .push(p1_regular(localized_copy(
                    "Spark is not configured for this cube yet. Set up a Spark \
                     signer and restart the app to connect the bridge.",
                )))
                .into(),
            SparkStatus::Loading => Column::new()
                .spacing(20)
                .push(p1_regular(localized_copy(
                    "Connecting to the Spark bridge…",
                )))
                .into(),
            SparkStatus::Error(err) => Column::new()
                .spacing(20)
                .push(p1_regular(localized_copy("Spark bridge error")))
                .push(p2_regular(err.clone()))
                .into(),
            SparkStatus::Connected(snapshot) => connected_view(
                snapshot.balance_sats,
                snapshot.usdb_balance.as_ref(),
                self.btc_usd_price,
                self.recent_transactions,
                self.fiat_converter,
                self.bitcoin_unit,
                self.show_direction_badges,
                self.display_mode,
                self.mask_amounts,
                self.cube_id,
                self.last_error,
                &self.lightning_address,
                self.lightning_share,
            ),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn connected_view<'a>(
    balance_sats: u64,
    usdb_balance: Option<&coincube_spark_protocol::UsdbBalanceSnapshot>,
    btc_usd_price: Option<f64>,
    recent: &'a [SparkRecentTransaction],
    fiat_converter: Option<FiatAmountConverter>,
    bitcoin_unit: BitcoinDisplayUnit,
    _show_direction_badges: bool,
    display_mode: DisplayMode,
    mask_amounts: bool,
    cube_id: &str,
    last_error: Option<&str>,
    lightning_address: &SparkLightningAddress,
    lightning_share: Option<(&'a str, Option<&'a qr_code::Data>)>,
) -> Element<'a, SparkOverviewMessage> {
    use crate::app::breez_spark::assets::stable_token_as_sats_u128;

    let mut content = Column::new().spacing(20);

    // Fold USDB into the unified portfolio total at the current
    // BTC/USD price — same pattern as Liquid's USDt fold. The
    // Stable Balance feature promises the spendable balance stays
    // pegged to fiat; if we showed only `balance_sats` here, toggling
    // Stable Balance ON would look like the wallet was emptied.
    //
    // USDB is USD-denominated. A quote in EUR/GBP is not a USD quote;
    // only the dedicated BTC/USD quote can value this holding in sats.
    let reference_price = btc_usd_price.filter(|price| *price > 0.0 && price.is_finite());
    let usdb_as_sats = usdb_balance
        .map(|sb| stable_token_as_sats_u128(sb.balance, sb.decimals, reference_price))
        .unwrap_or(0);
    let has_unvalued_usdb =
        usdb_balance.as_ref().is_some_and(|sb| sb.balance > 0) && reference_price.is_none();
    let total_balance = Amount::from_sat(balance_sats.saturating_add(usdb_as_sats));
    let total_fiat = (!has_unvalued_usdb)
        .then(|| fiat_converter.as_ref().map(|c| c.convert(total_balance)))
        .flatten();

    // Sum pending BTC (in/out) from the recent-transactions list for
    // the small "pending" indicators underneath the total. Skip
    // token-display rows so a USDB conversion entry doesn't get
    // counted as pending sats.
    let (pending_outgoing_sats, pending_incoming_sats) = pending_btc_sats(recent);

    // ── Unified portfolio card ─────────────────────────────────────────
    let total_col = Column::new()
        .spacing(4)
        .push(
            Row::new()
                .spacing(8)
                .align_y(Alignment::Center)
                .push(h4_bold(localized_copy("Total balance"))),
        )
        .push(if mask_amounts {
            amount_cell::<SparkOverviewMessage>(AmountCell::masked(), AmountTier::Card)
        } else if has_unvalued_usdb {
            amount_cell::<SparkOverviewMessage>(
                AmountCell::unavailable(coincube_ui::locale::copy(
                    "Combined BTC and USDB valuation needs a BTC/USD quote",
                )),
                AmountTier::Card,
            )
        } else {
            wallet_header::<SparkOverviewMessage>(WalletHeaderProps {
                sats: total_balance,
                fiat: total_fiat,
                balance_masked: mask_amounts,
                bitcoin_unit,
                variant: HeaderVariant::Landing,
                sync: SyncState::Synced,
                unconfirmed: None,
                pending_send_sats: pending_outgoing_sats,
                pending_receive_sats: pending_incoming_sats,
                display_mode,
                on_swap: Some(SparkOverviewMessage::FlipDisplayMode),
                on_swap_focus: Some((
                    landing_control_id(cube_id, "Spark", "flip-balance"),
                    SparkOverviewMessage::FocusAction,
                )),
            })
            .into()
        });

    // Each balance in its own unit: the total above is a current-rate
    // estimate only when a dedicated BTC/USD quote exists.
    let visibility = if mask_amounts {
        AmountVisibility::Hidden
    } else {
        AmountVisibility::Shown
    };
    let usdb_units = usdb_balance.map_or(0, |sb| sb.balance);
    let send_receive = |asset: &str, send: SparkOverviewMessage, receive: SparkOverviewMessage| {
        vec![
            landing_button(
                landing_control_id(cube_id, "Spark", &format!("send-{asset}")),
                "Send",
                None,
                SparkOverviewMessage::FocusAction,
                Some(send),
                theme::button::primary,
            )
            .width(Length::Fixed(90.0))
            .into(),
            landing_button(
                landing_control_id(cube_id, "Spark", &format!("receive-{asset}")),
                "Receive",
                None,
                SparkOverviewMessage::FocusAction,
                Some(receive),
                theme::button::orange_outline,
            )
            .width(Length::Fixed(90.0))
            .into(),
        ]
    };
    let mut rows = Column::new();
    for asset in spark_assets(balance_sats, usdb_units) {
        rows = rows.push(match asset {
            SparkAsset::Bitcoin => {
                let amount = Amount::from_sat(balance_sats);
                let primary = Figure::new(amount.to_formatted_string_with_unit(bitcoin_unit))
                    .unit(bitcoin_unit.label());
                let secondary = fiat_converter
                    .as_ref()
                    .map(|c| c.convert(amount))
                    .map(|f| Figure::new(f.to_formatted_string()).unit(f.currency().to_string()));
                asset_row(
                    coincube_ui::image::asset_network_logo::<SparkOverviewMessage>(
                        "btc",
                        "spark",
                        size::COIN,
                    ),
                    "Bitcoin",
                    if balance_sats == 0 {
                        BalanceValue::Zero { primary, secondary }
                    } else {
                        BalanceValue::Known { primary, secondary }
                    },
                    visibility,
                    send_receive(
                        "btc",
                        SparkOverviewMessage::SendBtc,
                        SparkOverviewMessage::ReceiveBtc,
                    ),
                )
            }
            SparkAsset::UsDollar => {
                use crate::app::breez_spark::assets::format_token_display_u128;
                let decimals = usdb_balance.map_or(0, |sb| sb.decimals);
                let primary =
                    Figure::new(format_token_display_u128(usdb_units, decimals)).unit("USDB");
                // ≈ sats only at a dedicated BTC/USD quote.
                let secondary = reference_price.map(|_| {
                    Figure::new(format!(
                        "≈ {}",
                        Amount::from_sat(usdb_as_sats).to_formatted_string_with_unit(bitcoin_unit)
                    ))
                    .unit(bitcoin_unit.label())
                });
                asset_row(
                    coincube_ui::image::asset_logo("usdb")
                        .width(Length::Fixed(size::COIN))
                        .height(Length::Fixed(size::COIN)),
                    "US Dollar",
                    BalanceValue::Known { primary, secondary },
                    visibility,
                    // Send opens Spark's Send paying from US Dollar; Receive
                    // opens Spark's Receive set to USDB sent on Spark.
                    send_receive(
                        "usdb",
                        SparkOverviewMessage::SendUsdb,
                        SparkOverviewMessage::ReceiveUsdb,
                    ),
                )
            }
        });
    }
    // The Lightning address sits beside the total while both fit at their
    // own widths, else on its own line below it (`fit_row`).
    let total_row: Element<'a, SparkOverviewMessage> =
        match lightning_address_button(cube_id, lightning_address, lightning_share.is_some()) {
            Some(button) => fit_row(total_col, button).gap(16.0).into(),
            None => total_col.into(),
        };
    // The share popover hangs from the button's trailing edge and covers the
    // rows below it rather than pushing them down.
    let total_row: Element<'a, SparkOverviewMessage> = match lightning_share {
        Some((address, qr)) => Column::new()
            .push(total_row)
            .push(
                Floating::new(lightning_share_panel(cube_id, address, qr))
                    .align_x(iced::alignment::Horizontal::Right)
                    .gap(8.0)
                    .on_dismiss(SparkOverviewMessage::CloseLightningShare),
            )
            .into(),
        None => total_row,
    };
    let balance_section = landing_balance_section(
        Column::new()
            .spacing(16)
            .push(total_row)
            .push(iced::widget::rule::horizontal(1))
            .push(rows),
    );

    content = content.push(balance_section);
    if let Some(error) = last_error {
        content = content.push(
            text(localized_format!(
                "Last known balance · Spark refresh failed: {error}",
                "Último saldo conocido · Spark refrescante falló: {error}"
            ))
            .size(P2_SIZE)
            .style(theme::text::warning),
        );
    }

    // "Recent activity" follows the balance; App mounts it from Activity
    // (`view::activity::recent_activity`).
    content = content.push(iced::widget::Space::new().height(Length::Fixed(20.0)));

    content.into()
}

fn pending_btc_sats(recent: &[SparkRecentTransaction]) -> (u64, u64) {
    let pending_outgoing_sats = recent
        .iter()
        .filter(|t| {
            !t.is_incoming
                && t.token_display.is_none()
                && matches!(t.status, DomainPaymentStatus::Pending)
        })
        .map(|t| t.display_amount().to_sat())
        .sum();
    let pending_incoming_sats = recent
        .iter()
        .filter(|t| {
            t.is_incoming
                && t.token_display.is_none()
                && matches!(t.status, DomainPaymentStatus::Pending)
        })
        .map(|t| t.display_amount().to_sat())
        .sum();

    (pending_outgoing_sats, pending_incoming_sats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::view;
    use crate::app::view::shared::landing::capture;

    const ADDRESS: &str = "sats@coincube.io";

    fn ready() -> SparkLightningAddress {
        SparkLightningAddress::Ready(ADDRESS.into())
    }

    /// The Spark page under its pinned head, at the top and scrolled (Tenshu
    /// v175 L3). Opt-in: `TOP_BAR_CAPTURE_DIR=<dir> cargo test -p
    /// coincube-gui --lib page_head_capture -- --ignored`.
    #[test]
    #[ignore = "writes PNG frames; needs a GPU adapter"]
    fn page_head_capture_spark() {
        use crate::app::menu::{Menu, SparkSubMenu};
        use crate::app::state::preferences::BalancePrivacy;
        use crate::app::view::nav::shell::capture_tests;
        let Some(directory) = capture_tests::directory() else {
            return;
        };
        let mut renderer = capture_tests::renderer();
        // Both balances, so the US Dollar row shows (W2).
        let snapshot = SparkBalanceSnapshot {
            balance_sats: 25_053,
            usdb_balance: Some(coincube_spark_protocol::UsdbBalanceSnapshot {
                token_identifier: "btkn1synthetic".into(),
                balance: 12_400_000,
                decimals: 6,
                ticker: "USDB".into(),
            }),
        };
        let menu = Menu::Spark(SparkSubMenu::Overview);
        // Enough "Recent activity" for the page to scroll under its head.
        let payments = (0..5)
            .map(|index: u32| coincube_spark_protocol::PaymentSummary {
                id: format!("capture-{index}"),
                amount_sat: 1_500 + 250 * i64::from(index),
                fees_sat: 0,
                token_amount: None,
                token_decimals: None,
                token_ticker: None,
                timestamp: 1_760_000_000 - 3_600 * u64::from(index),
                status: "completed".into(),
                direction: "receive".into(),
                method: "lightning".into(),
                description: Some(format!("Synthetic receipt {}", index + 1)),
                details: Default::default(),
            })
            .collect();
        for (appearance, palette) in capture_tests::themes() {
            let mut cache = capture_tests::cache(appearance == "light", BalancePrivacy::Shown);
            cache.wallet_names.spark = Some("Daily spending".into());
            let mut activity = crate::app::state::activity::Activity::new(None, None);
            drop(crate::app::state::State::update(
                &mut activity,
                None,
                &cache,
                crate::app::message::Message::ActivitySparkLoaded(
                    0,
                    Ok((Vec::clone(&payments), false)),
                ),
            ));
            let (cache, activity, snapshot) = (&cache, &activity, &snapshot);
            // As App mounts the page (`state::spark::overview`
            // `view_with_lightning_share`, `page_body`).
            capture_tests::save_page_head_frames(
                &mut renderer,
                &directory,
                "spark",
                appearance,
                &palette,
                || {
                    view::dashboard_with_detail(
                        &menu,
                        cache,
                        move || {
                            crate::app::state::spark::overview::SparkOverview::page_head_for_tests(
                                cache,
                                Some(crate::app::view::global_home::WalletTransferEntry::Ready),
                            )
                        },
                        move || {
                            Column::new()
                                .push(
                                    synthetic_overview_with(
                                        SparkStatus::Connected(snapshot.clone()),
                                        &[],
                                        Some(80_000.0),
                                        None,
                                        ready(),
                                    )
                                    .map(view::Message::SparkOverview),
                                )
                                .push(view::activity::recent_activity(
                                    activity,
                                    cache,
                                    view::activity::RecentScope::Wallet(
                                        view::activity::WalletFilter::Spark,
                                    ),
                                    None,
                                ))
                                .into()
                        },
                        || None,
                    )
                },
            );
        }
    }

    #[test]
    fn spark_page_offers_its_lightning_address_beside_the_balance() {
        let snapshot = SparkBalanceSnapshot {
            balance_sats: 25_053,
            usdb_balance: None,
        };
        let share = landing_control_id("synthetic-cube", "Spark", "ln-share");
        let claim = landing_control_id("synthetic-cube", "Spark", "ln-claim");
        for (appearance, theme) in [
            ("dark", theme::Theme::dark()),
            ("light", theme::Theme::light()),
        ] {
            for size in [(1024, 768), (1440, 900)] {
                let page = |address| {
                    synthetic_overview_with(
                        SparkStatus::Connected(snapshot.clone()),
                        &[],
                        None,
                        None,
                        address,
                    )
                };
                let mounted = capture::inspect(
                    "spark-address-ready",
                    page(ready()),
                    size,
                    appearance,
                    theme,
                );
                assert!(mounted.contains("Total balance"));
                assert!(mounted.contains(ADDRESS));
                assert!(mounted.contains("Your Lightning address · share it to get paid"));
                assert!(mounted.contains("Share"));
                assert!(mounted.has_focus(share.clone()));
                // The old card below the activity, and its inline editing,
                // are gone from the page.
                assert!(!mounted.contains("Receive to a readable address"));
                assert!(!mounted.contains("Change your Lightning Address"));
                for control in ["ln-copy", "ln-change"] {
                    assert!(!mounted.has_focus(landing_control_id(
                        "synthetic-cube",
                        "Spark",
                        control
                    )));
                }
                for pointer in [false, true] {
                    let emitted = capture::activate(page(ready()), share.clone(), size, pointer);
                    assert!(
                        matches!(
                            emitted.last(),
                            Some(SparkOverviewMessage::ShareLightningAddress(address)) if address == ADDRESS
                        ),
                        "{:?}",
                        emitted
                    );
                }

                let unclaimed = capture::inspect(
                    "spark-address-unclaimed",
                    page(SparkLightningAddress::Unclaimed),
                    size,
                    appearance,
                    theme,
                );
                assert!(unclaimed.contains("Claim your Lightning address"));
                assert!(!unclaimed.contains("Share"));
                assert!(!unclaimed.has_focus(share.clone()));
                for pointer in [false, true] {
                    let emitted = capture::activate(
                        page(SparkLightningAddress::Unclaimed),
                        claim.clone(),
                        size,
                        pointer,
                    );
                    assert!(
                        matches!(
                            emitted.last(),
                            Some(SparkOverviewMessage::ClaimLightningAddress)
                        ),
                        "{:?}",
                        emitted
                    );
                }

                // Unknown until the Cube's registration answers: no button
                // that would flip from "Claim" to the address.
                let unknown = capture::inspect(
                    "spark-address-unknown",
                    page(SparkLightningAddress::Unknown),
                    size,
                    appearance,
                    theme,
                );
                assert!(unknown.contains("Total balance"));
                assert!(!unknown.contains("Lightning address"));
                assert!(!unknown.has_focus(share.clone()));
                assert!(!unknown.has_focus(claim.clone()));
            }
        }
    }

    #[test]
    fn lightning_share_panel_offers_the_qr_copy_save_and_change() {
        let qr = qr_code::Data::new(ADDRESS).expect("address fits a QR code");
        let copy = landing_control_id("synthetic-cube", "Spark", "ln-copy");
        let change = landing_control_id("synthetic-cube", "Spark", "ln-change");
        let save = landing_control_id("synthetic-cube", "Spark", "ln-save-qr");
        for (appearance, theme) in [
            ("dark", theme::Theme::dark()),
            ("light", theme::Theme::light()),
        ] {
            let panel = || lightning_share_panel("synthetic-cube", ADDRESS, Some(&qr));
            let mounted =
                capture::inspect("spark-share-panel", panel(), (1024, 768), appearance, theme);
            for line in [
                "Get paid over Lightning",
                "Anyone with a Lightning wallet can send you bitcoin at this address, even \
                 while Tenshu is closed.",
                ADDRESS,
                "Copy address",
                "Want a different name?",
                "Change address",
            ] {
                assert!(mounted.contains(line), "{}", line);
            }
            assert!(!mounted.contains("QR unavailable"));
            assert!(mounted.contains("Save QR image"));
            assert!(mounted.has_focus(copy.clone()));
            assert!(mounted.has_focus(save.clone()));
            assert!(mounted.has_focus(change.clone()));
            for pointer in [false, true] {
                let copied = capture::activate(panel(), copy.clone(), (1024, 768), pointer);
                assert!(
                    matches!(
                        copied.last(),
                        Some(SparkOverviewMessage::CopyLightningAddress(address)) if address == ADDRESS
                    ),
                    "{:?}",
                    copied
                );
                let saved = capture::activate(panel(), save.clone(), (1024, 768), pointer);
                assert!(
                    matches!(
                        saved.last(),
                        Some(SparkOverviewMessage::SaveLightningQr(address)) if address == ADDRESS
                    ),
                    "{:?}",
                    saved
                );
                let changed = capture::activate(panel(), change.clone(), (1024, 768), pointer);
                assert!(
                    matches!(
                        changed.last(),
                        Some(SparkOverviewMessage::ChangeLightningAddress)
                    ),
                    "{:?}",
                    changed
                );
            }
            let without_qr = capture::inspect(
                "spark-share-panel-no-qr",
                lightning_share_panel("synthetic-cube", ADDRESS, None),
                (1024, 768),
                appearance,
                theme,
            );
            assert!(without_qr.contains("QR unavailable"));
            assert!(without_qr.contains(ADDRESS));
            assert!(!without_qr.contains("Save QR image"));
        }
    }

    #[test]
    fn lightning_share_hangs_under_the_address_button() {
        use iced::advanced::{
            clipboard::Null, layout, overlay, renderer::Headless, widget::Tree, Layout, Shell,
        };
        use iced::{keyboard, mouse, Event, Point, Rectangle, Size, Vector};

        for bytes in coincube_ui::font::load() {
            iced::advanced::graphics::text::font_system()
                .write()
                .expect("font system")
                .load_font(bytes);
        }

        let qr = qr_code::Data::new(ADDRESS).expect("address fits a QR code");
        let snapshot = SparkBalanceSnapshot {
            balance_sats: 25_053,
            usdb_balance: None,
        };
        let page = |share: bool| -> Element<'_, SparkOverviewMessage> {
            let body = SparkOverviewView {
                lightning_share: share.then_some((ADDRESS, Some(&qr))),
                ..synthetic_view(
                    SparkStatus::Connected(snapshot.clone()),
                    &[],
                    None,
                    None,
                    ready(),
                )
            }
            .render();
            iced::widget::scrollable(body)
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
        };
        for (appearance, theme) in [
            ("dark", theme::Theme::dark()),
            ("light", theme::Theme::light()),
        ] {
            for (width, height) in [(1024, 768), (1440, 900)] {
                let size = Size::new(width as f32, height as f32);
                let viewport = Rectangle::with_size(size);
                let mut renderer =
                    iced::futures::executor::block_on(<iced::Renderer as Headless>::new(
                        coincube_ui::font::REGULAR,
                        iced::Pixels(14.0),
                        Some("tiny-skia"),
                    ))
                    .expect("software renderer");
                let limits = layout::Limits::new(Size::ZERO, size);

                // The closed page, for the layout the popover must not move.
                let mut closed = page(false);
                let mut closed_tree = Tree::new(closed.as_widget());
                let closed_node =
                    closed
                        .as_widget_mut()
                        .layout(&mut closed_tree, &renderer, &limits);
                let closed_text =
                    capture_text(&mut closed, &mut closed_tree, &closed_node, &renderer);

                let mut element = page(true);
                let mut tree = Tree::new(element.as_widget());
                let node = element
                    .as_widget_mut()
                    .layout(&mut tree, &renderer, &limits);
                let open_text = capture_text(&mut element, &mut tree, &node, &renderer);
                for entry in &closed_text {
                    assert!(
                        open_text.contains(entry),
                        "the popover moves nothing on the page: {:?}",
                        entry
                    );
                }

                let button = closed_text
                    .iter()
                    .find(|(text, _)| text == "Share")
                    .map(|(_, bounds)| *bounds)
                    .expect("Share on the address button");
                let mut popover = element
                    .as_widget_mut()
                    .overlay(
                        &mut tree,
                        Layout::new(&node),
                        &renderer,
                        &viewport,
                        Vector::ZERO,
                    )
                    .map(overlay::Nested::new)
                    .expect("share popover overlay");
                let overlay_node = popover.layout(&renderer, size);
                // Overlays from several widgets come back as one page-wide
                // group; the popover is the tallest node narrower than it.
                fn narrow_nodes(node: &layout::Node, width: f32, found: &mut Vec<Rectangle>) {
                    if node.bounds().width < width {
                        found.push(node.bounds());
                    } else {
                        for child in node.children() {
                            narrow_nodes(child, width, found);
                        }
                    }
                }
                let mut narrow = Vec::new();
                narrow_nodes(&overlay_node, size.width, &mut narrow);
                let panel = narrow
                    .into_iter()
                    .max_by(|a, b| a.height.total_cmp(&b.height))
                    .expect("popover node");
                assert_eq!(panel.width, 380.0, "the popover keeps its own width");
                assert!(panel.height < size.height / 1.2, "not a whole-page scrim");
                assert!(panel.y > button.y + button.height, "hangs below the button");
                assert!(panel.y < button.y + button.height + 40.0, "close under it");
                assert!(
                    (panel.x + panel.width) > button.x + button.width
                        && (panel.x + panel.width) <= size.width,
                    "trailing edge under the button's: {:?} {:?}",
                    panel,
                    button
                );

                let mut send = |event: Event, at: Point| {
                    let mut messages = Vec::new();
                    let mut shell = Shell::new(&mut messages);
                    popover.update(
                        &event,
                        Layout::new(&overlay_node),
                        mouse::Cursor::Available(at),
                        &renderer,
                        &mut Null,
                        &mut shell,
                    );
                    let captured = shell.is_event_captured();
                    (messages, captured)
                };
                let press = Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left));
                let (inside, _) = send(press.clone(), panel.center());
                assert!(inside.is_empty(), "{:?}", inside);
                let (outside, captured) = send(press, Point::new(10.0, size.height - 10.0));
                assert!(captured, "a dismissing press goes no further");
                assert!(matches!(
                    outside.as_slice(),
                    [SparkOverviewMessage::CloseLightningShare]
                ));
                let escape = Event::Keyboard(keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(keyboard::key::Named::Escape),
                    modified_key: keyboard::Key::Named(keyboard::key::Named::Escape),
                    physical_key: keyboard::key::Physical::Code(keyboard::key::Code::Escape),
                    location: keyboard::Location::Standard,
                    modifiers: keyboard::Modifiers::default(),
                    text: None,
                    repeat: false,
                });
                let (escaped, _) = send(escape, panel.center());
                assert!(matches!(
                    escaped.as_slice(),
                    [SparkOverviewMessage::CloseLightningShare]
                ));
                drop(popover);

                if let Ok(directory) = std::env::var("R06A_CAPTURE_DIR") {
                    std::fs::create_dir_all(&directory).expect("capture directory");
                    iced::advanced::Renderer::reset(&mut renderer, viewport);
                    let style = iced::advanced::renderer::Style {
                        text_color: theme.colors.text.primary,
                    };
                    element.as_widget().draw(
                        &tree,
                        &mut renderer,
                        &theme,
                        &style,
                        Layout::new(&node),
                        mouse::Cursor::Unavailable,
                        &viewport,
                    );
                    if let Some(mut popover) = element
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
                        let overlay_node = popover.layout(&renderer, size);
                        popover.draw(
                            &mut renderer,
                            &theme,
                            &style,
                            Layout::new(&overlay_node),
                            mouse::Cursor::Unavailable,
                        );
                    }
                    let pixels = renderer.screenshot(
                        Size::new(width, height),
                        1.0,
                        theme.colors.general.background,
                    );
                    image::save_buffer(
                        std::path::Path::new(&directory).join(format!(
                            "spark-share-popover-{appearance}-{width}x{height}-synthetic.png"
                        )),
                        &pixels,
                        width,
                        height,
                        image::ColorType::Rgba8,
                    )
                    .expect("save popover capture");
                }
            }
        }
    }

    /// Every text on the page with its bounds, popover content included.
    fn capture_text(
        element: &mut Element<'_, SparkOverviewMessage>,
        tree: &mut iced::advanced::widget::Tree,
        node: &iced::advanced::layout::Node,
        renderer: &iced::Renderer,
    ) -> Vec<(String, iced::Rectangle)> {
        use iced::advanced::{widget::operation::Operation, Layout};

        #[derive(Default)]
        struct Texts(Vec<(String, iced::Rectangle)>);
        impl Operation for Texts {
            fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
                operate(self);
            }
            fn text(
                &mut self,
                _id: Option<&iced::advanced::widget::Id>,
                bounds: iced::Rectangle,
                text: &str,
            ) {
                self.0.push((text.to_owned(), bounds));
            }
        }
        let mut texts = Texts::default();
        element
            .as_widget_mut()
            .operate(tree, Layout::new(node), renderer, &mut texts);
        texts.0
    }

    fn tx(
        id: &str,
        is_incoming: bool,
        status: DomainPaymentStatus,
        amount_sat: u64,
        fees_sat: u64,
        token_display: Option<&str>,
    ) -> SparkRecentTransaction {
        SparkRecentTransaction {
            id: id.to_string(),
            description: id.to_string(),
            time_ago: "just now".to_string(),
            timestamp: 1_700_000_000,
            amount: Amount::from_sat(amount_sat),
            fees_sat: Amount::from_sat(fees_sat),
            fiat_amount: None,
            is_incoming,
            status,
            method: if token_display.is_some() {
                SparkPaymentMethod::StableBalance
            } else {
                SparkPaymentMethod::Spark
            },
            token_display: token_display.map(str::to_string),
        }
    }

    #[test]
    fn pending_btc_sats_counts_pending_incoming_and_outgoing_totals() {
        let rows = vec![
            tx(
                "incoming",
                true,
                DomainPaymentStatus::Pending,
                1_000,
                12,
                None,
            ),
            tx(
                "outgoing",
                false,
                DomainPaymentStatus::Pending,
                2_000,
                34,
                None,
            ),
        ];

        assert_eq!(pending_btc_sats(&rows), (2_034, 1_000));
    }

    #[test]
    fn pending_btc_sats_skips_completed_failed_and_token_rows() {
        let rows = vec![
            tx(
                "complete",
                true,
                DomainPaymentStatus::Complete,
                1_000,
                0,
                None,
            ),
            tx(
                "failed",
                false,
                DomainPaymentStatus::Failed,
                2_000,
                40,
                None,
            ),
            tx(
                "token",
                true,
                DomainPaymentStatus::Pending,
                999_999,
                0,
                Some("1.23 USDB"),
            ),
        ];

        assert_eq!(pending_btc_sats(&rows), (0, 0));
    }

    #[test]
    fn display_amount_adds_fees_only_for_outgoing_rows() {
        assert_eq!(
            tx("incoming", true, DomainPaymentStatus::Pending, 100, 9, None)
                .display_amount()
                .to_sat(),
            100
        );
        assert_eq!(
            tx(
                "outgoing",
                false,
                DomainPaymentStatus::Pending,
                100,
                9,
                None
            )
            .display_amount()
            .to_sat(),
            109
        );
    }

    fn synthetic_overview<'a>(
        status: SparkStatus,
        rows: &'a [SparkRecentTransaction],
        price: Option<f64>,
        last_error: Option<&'a str>,
    ) -> Element<'a, SparkOverviewMessage> {
        synthetic_overview_with(
            status,
            rows,
            price,
            last_error,
            SparkLightningAddress::Unknown,
        )
    }

    fn synthetic_overview_with<'a>(
        status: SparkStatus,
        rows: &'a [SparkRecentTransaction],
        price: Option<f64>,
        last_error: Option<&'a str>,
        lightning_address: SparkLightningAddress,
    ) -> Element<'a, SparkOverviewMessage> {
        synthetic_view(status, rows, price, last_error, lightning_address).render()
    }

    fn synthetic_view<'a>(
        status: SparkStatus,
        rows: &'a [SparkRecentTransaction],
        price: Option<f64>,
        last_error: Option<&'a str>,
        lightning_address: SparkLightningAddress,
    ) -> SparkOverviewView<'a> {
        SparkOverviewView {
            status,
            recent_transactions: rows,
            cube_id: "synthetic-cube",
            last_error,
            fiat_converter: None,
            bitcoin_unit: BitcoinDisplayUnit::Sats,
            show_direction_badges: true,
            btc_usd_price: price,
            display_mode: DisplayMode::BitcoinNative,
            mask_amounts: false,
            lightning_address,
            lightning_share: None,
        }
    }

    #[test]
    fn hidden_overview_redacts_btc_usdb_and_activity_amounts() {
        let stable = coincube_spark_protocol::UsdbBalanceSnapshot {
            token_identifier: "btkn1synthetic".into(),
            balance: 2_500_000,
            decimals: 6,
            ticker: "USDB".into(),
        };
        let snapshot = SparkBalanceSnapshot {
            balance_sats: 12_345_678,
            usdb_balance: Some(stable),
        };
        let rows = [tx(
            "payment",
            true,
            DomainPaymentStatus::Complete,
            98_765_432,
            0,
            None,
        )];
        let mounted = capture::inspect(
            "spark-hidden-amounts",
            SparkOverviewView {
                status: SparkStatus::Connected(snapshot),
                recent_transactions: &rows,
                cube_id: "synthetic-cube",
                last_error: None,
                fiat_converter: None,
                bitcoin_unit: BitcoinDisplayUnit::Sats,
                show_direction_badges: true,
                btc_usd_price: Some(50_000.0),
                display_mode: DisplayMode::BitcoinNative,
                mask_amounts: true,
                lightning_address: ready(),
                lightning_share: None,
            }
            .render(),
            (1024, 768),
            "dark",
            theme::Theme::dark(),
        );
        assert!(mounted.contains("••••••"));
        for amount in ["12,345,678", "98,765,432", "2.50"] {
            assert!(!mounted.contains(amount), "hidden Spark amount {}", amount);
        }
        // An address is not an amount: hiding balances leaves it readable.
        assert!(mounted.contains(ADDRESS));
    }

    #[test]
    fn r06a_spark_headless_content_matrix() {
        let stable = coincube_spark_protocol::UsdbBalanceSnapshot {
            token_identifier: "btkn1synthetic".into(),
            balance: 2_500_000,
            decimals: 6,
            ticker: "USDB".into(),
        };
        let snapshot = SparkBalanceSnapshot {
            balance_sats: 125_000,
            usdb_balance: Some(stable),
        };
        let rows = vec![
            tx(
                "payment-a",
                true,
                DomainPaymentStatus::Complete,
                5_000,
                0,
                None,
            ),
            tx(
                "payment-b",
                false,
                DomainPaymentStatus::Pending,
                7_000,
                15,
                None,
            ),
        ];
        for (appearance, theme) in [
            ("dark", theme::Theme::dark()),
            ("light", theme::Theme::light()),
        ] {
            for outer in [(1024, 768), (1440, 900)] {
                let normal = capture::inspect(
                    "spark-normal",
                    synthetic_overview(
                        SparkStatus::Connected(snapshot.clone()),
                        &rows,
                        Some(50_000.0),
                        None,
                    ),
                    outer,
                    appearance,
                    theme,
                );
                // "Recent activity" is Activity's, mounted by App.
                assert!(!normal.contains("Last transactions"));
                assert!(normal.contains("Total balance"));
                for label in ["Bitcoin", "US Dollar", "2.50", "USDB"] {
                    assert!(normal.contains(label), "{}", label);
                }
                // 2.50 USDB at 50,000 USD/BTC is 5,000 sats.
                assert!(normal.contains("≈ 5,000"));
                assert!(!normal.contains("Stable"));
                for control in [
                    "flip-balance",
                    "send-btc",
                    "receive-btc",
                    "send-usdb",
                    "receive-usdb",
                ] {
                    assert!(normal.has_focus(landing_control_id(
                        "synthetic-cube",
                        "Spark",
                        control
                    )));
                }
                let missing_quote = capture::inspect(
                    "spark-missing-quote-held-off",
                    synthetic_overview(
                        SparkStatus::Connected(snapshot.clone()),
                        &rows,
                        None,
                        Some("bridge refresh failed"),
                    ),
                    outer,
                    appearance,
                    theme,
                );
                assert!(missing_quote.contains("US Dollar"));
                assert!(!missing_quote.contains("≈"), "no sats without a quote");
                assert!(missing_quote.contains("needs a BTC/USD quote"));
                assert!(missing_quote.contains("Last known balance"));
                assert!(!missing_quote.contains("Stable"));
                let unavailable = capture::inspect(
                    "spark-bridge-unavailable",
                    synthetic_overview(SparkStatus::Unavailable, &[], None, None),
                    outer,
                    appearance,
                    theme,
                );
                assert!(unavailable.contains("not configured"));
            }
        }
    }

    /// Bitcoin always, unless the wallet holds only USDB; US Dollar while it
    /// holds any.
    #[test]
    fn spark_lists_a_row_per_balance_it_holds() {
        use SparkAsset::*;
        assert_eq!(spark_assets(0, 0), [Bitcoin], "an empty wallet");
        assert_eq!(spark_assets(25_053, 0), [Bitcoin]);
        assert_eq!(spark_assets(25_053, 2_500_000), [Bitcoin, UsDollar]);
        assert_eq!(spark_assets(0, 2_500_000), [UsDollar], "USDB only");
    }
}

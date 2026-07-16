use serde::{Deserialize, Serialize};

/// Supported external USDt networks (beyond native Liquid).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SideshiftNetwork {
    Liquid,
    Ethereum,
    Tron,
    Binance,
    Solana,
}

impl SideshiftNetwork {
    /// Human-readable display name shown in the UI (includes "USDt").
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Liquid => "Liquid USDt",
            Self::Ethereum => "Ethereum USDt",
            Self::Tron => "Tron USDt",
            Self::Binance => "Binance USDt",
            Self::Solana => "Solana USDt",
        }
    }

    /// Just the network name without "USDt".
    pub fn network_name(&self) -> &'static str {
        match self {
            Self::Liquid => "Liquid",
            Self::Ethereum => "Ethereum",
            Self::Tron => "Tron",
            Self::Binance => "Binance",
            Self::Solana => "Solana",
        }
    }

    /// Subtitle shown beneath external network options in the picker.
    pub fn swap_subtitle(&self) -> Option<&'static str> {
        match self {
            Self::Liquid => None,
            _ => Some("Swapped to Liquid USDt"),
        }
    }

    /// SideShift API `depositNetwork` / `settleNetwork` slug.
    pub fn network_slug(&self) -> &'static str {
        match self {
            Self::Liquid => "liquid",
            Self::Ethereum => "ethereum",
            Self::Tron => "tron",
            Self::Binance => "bsc",
            Self::Solana => "solana",
        }
    }

    /// Short standard label shown in the "Only for …" warning badge.
    pub fn standard_label(&self) -> &'static str {
        match self {
            Self::Liquid => "Liquid",
            Self::Ethereum => "ERC-20",
            Self::Tron => "TRC-20",
            Self::Binance => "BEP-20",
            Self::Solana => "SPL",
        }
    }

    /// Returns `true` if every character in `s` is a valid base58 character
    /// (alphanumeric excluding `0`, `O`, `I`, `l`).
    fn is_base58(s: &str) -> bool {
        s.bytes().all(|b| {
            matches!(b,
                b'1'..=b'9'
                | b'A'..=b'H'
                | b'J'..=b'N'
                | b'P'..=b'Z'
                | b'a'..=b'k'
                | b'm'..=b'z'
            )
        })
    }

    /// Detect possible networks from a recipient address.
    /// Returns empty vec for unrecognised formats.
    pub fn detect_from_address(addr: &str) -> Vec<SideshiftNetwork> {
        let addr = addr.trim();
        if addr.is_empty() {
            return vec![];
        }

        // Liquid: confidential (VJL/VTp/VTq), blech32 (lq1/ex1), or unconfidential (Q/G/H, exactly 34 chars)
        if addr.starts_with("VJL")
            || addr.starts_with("VTp")
            || addr.starts_with("VTq")
            || addr.starts_with("lq1")
            || addr.starts_with("ex1")
            || (addr.len() == 34
                && (addr.starts_with('Q') || addr.starts_with('G') || addr.starts_with('H')))
        {
            return vec![Self::Liquid];
        }

        // Tron: starts with T, base58, 34 chars
        if addr.starts_with('T') && addr.len() == 34 && Self::is_base58(addr) {
            return vec![Self::Tron];
        }

        // EVM-compatible: 0x + 40 hex chars → ambiguous (Ethereum, Binance)
        if addr.starts_with("0x")
            && addr.len() == 42
            && addr[2..].chars().all(|c| c.is_ascii_hexdigit())
        {
            return vec![Self::Ethereum, Self::Binance];
        }

        // Solana: base58-encoded 32-byte Ed25519 public key (always 43–44 chars).
        if (43..=44).contains(&addr.len()) && Self::is_base58(addr) {
            return vec![Self::Solana];
        }

        vec![]
    }

    /// Returns all networks in display order.
    pub fn all() -> &'static [SideshiftNetwork] {
        &[
            Self::Liquid,
            Self::Ethereum,
            Self::Tron,
            Self::Binance,
            Self::Solana,
        ]
    }
}

// ---------------------------------------------------------------------------
// API request/response types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuoteRequest {
    pub deposit_coin: String,
    pub deposit_network: String,
    pub settle_coin: String,
    pub settle_network: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deposit_amount: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settle_amount: Option<String>,
    pub affiliate_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShiftQuote {
    pub id: String,
    pub deposit_coin: String,
    pub deposit_network: String,
    pub settle_coin: String,
    pub settle_network: String,
    pub deposit_amount: Option<String>,
    pub settle_amount: Option<String>,
    pub rate: Option<String>,
    pub affiliate_id: Option<String>,
    pub created_at: Option<String>,
    pub expires_at: Option<String>,
    pub min: Option<String>,
    pub max: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FixedShiftRequest {
    pub quote_id: String,
    pub settle_address: String,
    pub affiliate_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VariableShiftRequest {
    pub deposit_coin: String,
    pub deposit_network: String,
    pub settle_coin: String,
    pub settle_network: String,
    pub settle_address: String,
    pub affiliate_id: String,
    /// Where SideShift returns the deposit if the shift can't settle — the
    /// user's address **on the origin chain**, not on Bitcoin.
    ///
    /// Optional to SideShift, mandatory in our flows. Without it, a shift that
    /// fails, expires, or gets held for review has nowhere to send the money
    /// back to, and the user's funds sit with a third party. It's collected up
    /// front, before any deposit address is shown, precisely because it is
    /// useless to ask for afterwards.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refund_address: Option<String>,
}

/// Response returned by both fixed and variable shift creation endpoints.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShiftResponse {
    pub id: String,
    pub deposit_address: String,
    pub deposit_coin: Option<String>,
    pub deposit_network: Option<String>,
    pub settle_address: Option<String>,
    pub settle_coin: Option<String>,
    pub settle_network: Option<String>,
    pub deposit_min: Option<String>,
    pub deposit_max: Option<String>,
    pub deposit_amount: Option<String>,
    pub settle_amount: Option<String>,
    pub rate: Option<String>,
    pub expires_at: Option<String>,
    pub created_at: Option<String>,
    pub status: Option<String>,
    pub affiliate_fee_percent: Option<String>,
    pub network_fee_usd: Option<String>,
}

/// Status response from `GET /v2/shifts/{shiftId}`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShiftStatus {
    pub id: String,
    pub status: String,
    pub deposit_address: Option<String>,
    pub deposit_amount: Option<String>,
    pub settle_amount: Option<String>,
    pub expires_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShiftStatusKind {
    Waiting,
    Pending,
    Processing,
    /// SideShift has flagged the shift and is holding it for manual review
    /// (their compliance checks). **Not** a transient processing state: it can
    /// sit here indefinitely and needs the user to contact SideShift.
    ///
    /// This has its own variant because rendering it as a spinner would be a
    /// lie — the user would sit watching a "nearly there" animation for a
    /// shift that is going nowhere without them acting.
    Review,
    Settling,
    Settled,
    Expired,
    /// The refund is on its way back to the origin-chain address.
    Refunding,
    Refunded,
    Error,
    Unknown(String),
}

impl From<&str> for ShiftStatusKind {
    fn from(s: &str) -> Self {
        match s {
            "waiting" => Self::Waiting,
            "pending" => Self::Pending,
            "processing" => Self::Processing,
            "review" => Self::Review,
            "settling" => Self::Settling,
            "settled" => Self::Settled,
            "expired" => Self::Expired,
            "refunding" => Self::Refunding,
            "refunded" | "refund" => Self::Refunded,
            "error" => Self::Error,
            other => Self::Unknown(other.to_string()),
        }
    }
}

impl ShiftStatusKind {
    pub fn display(&self) -> &str {
        match self {
            Self::Waiting => "Waiting for deposit",
            Self::Pending => "Deposit detected",
            Self::Processing => "Processing",
            Self::Review => "On hold for review",
            Self::Settling => "Settling",
            Self::Settled => "Settled",
            Self::Expired => "Expired",
            Self::Refunding => "Refunding",
            Self::Refunded => "Refunded",
            Self::Error => "Error",
            Self::Unknown(_) => "Unknown",
        }
    }

    /// Whether the shift has stopped moving on its own.
    ///
    /// [`Self::Review`] counts: SideShift will not advance a held shift without
    /// human intervention, so a poller that treats it as in-flight spins
    /// forever and the UI never tells the user they have to do something.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Settled | Self::Expired | Self::Refunded | Self::Error | Self::Review
        )
    }

    /// Whether this state needs the user to act, as opposed to just waiting.
    /// Drives a call-to-action rather than a progress indicator.
    pub fn needs_user_action(&self) -> bool {
        matches!(self, Self::Review | Self::Error)
    }

    /// The sentence shown under the status. Held and failed shifts get
    /// something actionable; the rest get an expectation-setter.
    pub fn guidance(&self) -> &'static str {
        match self {
            Self::Review => {
                "SideShift has placed this shift on hold for review. It won't complete on its \
                 own — contact SideShift with your shift ID. If it can't be released, it will \
                 be refunded to the address you gave."
            }
            Self::Error => {
                "This shift failed. Any deposit will be returned to your refund address."
            }
            Self::Expired => {
                "This shift expired before a deposit arrived. Nothing was sent, and no funds \
                 are at risk."
            }
            Self::Refunding => {
                "The deposit is being returned to the refund address you gave, on its \
                 original network."
            }
            // Distinct from `Refunding`: this one is finished. Telling a user
            // their money "is being returned" when it already has been sends
            // them looking for a transfer that has, in fact, arrived.
            Self::Refunded => {
                "The deposit has been returned to the refund address you gave, on its \
                 original network."
            }
            Self::Settled => "Your bitcoin has arrived in your Spark wallet.",
            // The deposit is on-chain BTC into Spark's static address, so it
            // lands after a confirmation — not instantly. Say so, or a user
            // watching a spinner will assume something has gone wrong.
            Self::Settling | Self::Processing | Self::Pending => {
                "Converting to bitcoin and sending it to your Spark wallet. This arrives \
                 after about one confirmation, so it may take a few minutes."
            }
            Self::Waiting => "Send the exact asset and network shown above to the deposit address.",
            Self::Unknown(_) => "Checking the status of this shift.",
        }
    }
}

/// Backend response for `GET /api/v1/config/sideshift`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SideshiftConfig {
    pub affiliate_id: String,
}

/// SideShift's coin slug for Bitcoin, and the network it settles on. The Spark
/// receive bridge always settles here: SideShift offers no Lightning rail, so
/// the destination is the Spark wallet's on-chain Bitcoin deposit address.
pub const BTC_COIN: &str = "btc";
pub const BTC_NETWORK: &str = "bitcoin";

/// One thing a user can deposit in the Spark "receive from another network"
/// flow — an (asset, network) pair, because an asset alone is ambiguous: USDT
/// exists on Ethereum, Tron, BSC and Solana, and they are not interchangeable.
///
/// A curated list rather than SideShift's full catalogue. Every entry here is a
/// pair we can also *refund* correctly, since [`validate_refund_address`] keys
/// off [`SideshiftNetwork`] — offering a deposit we can't validate a refund
/// address for would be offering a way to lose money. Widening the list means
/// widening that validation first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DepositOption {
    /// SideShift's `depositCoin` slug.
    pub coin: &'static str,
    /// The chain it's deposited on. Also the chain a refund goes back to.
    pub network: SideshiftNetwork,
    /// What the user sees in the picker.
    pub label: &'static str,
}

impl DepositOption {
    /// A stable key for this pair, used to round-trip a selection through a
    /// message without carrying the whole struct.
    pub fn key(&self) -> String {
        format!("{}:{}", self.coin, self.network.network_slug())
    }
}

/// Everything the Spark receive bridge accepts as a deposit.
pub fn deposit_options() -> &'static [DepositOption] {
    &[
        DepositOption {
            coin: "usdt",
            network: SideshiftNetwork::Ethereum,
            label: "USDt on Ethereum (ERC-20)",
        },
        DepositOption {
            coin: "usdt",
            network: SideshiftNetwork::Tron,
            label: "USDt on Tron (TRC-20)",
        },
        DepositOption {
            coin: "usdt",
            network: SideshiftNetwork::Binance,
            label: "USDt on BNB Chain (BEP-20)",
        },
        DepositOption {
            coin: "usdt",
            network: SideshiftNetwork::Solana,
            label: "USDt on Solana (SPL)",
        },
        // Liquid USDt has no "-20" token standard — it's a native Liquid
        // asset, so no parenthetical. Natural for users winding down a
        // grandfathered Liquid wallet who want their USDt as Spark BTC.
        DepositOption {
            coin: "usdt",
            network: SideshiftNetwork::Liquid,
            label: "USDt on Liquid",
        },
        DepositOption {
            coin: "usdc",
            network: SideshiftNetwork::Ethereum,
            label: "USDC on Ethereum (ERC-20)",
        },
        DepositOption {
            coin: "usdc",
            network: SideshiftNetwork::Solana,
            label: "USDC on Solana (SPL)",
        },
        // Stablecoins only. A Bitcoin wallet converting USD stablecoins to BTC is
        // a defensible on-ramp; offering Ether/BNB/SOL/TRX → BTC is a generic
        // altcoin swap that dilutes focus and invites unbounded coin-list creep.
        // If those are ever wanted back, re-add them here (the flow is
        // coin-agnostic).
    ]
}

/// Resolve a [`DepositOption::key`] back to the option it names.
pub fn deposit_option_by_key(key: &str) -> Option<DepositOption> {
    deposit_options().iter().copied().find(|o| o.key() == key)
}

/// Validate a refund address against the network it will be refunded *on*.
///
/// The refund goes back to the **origin** chain — if you deposit USDT on Tron,
/// a failed shift refunds Tron, not Bitcoin. Users reliably get this wrong and
/// paste their Bitcoin address, which would send the refund into the void. So
/// the address is checked against the deposit network, not the settle network.
///
/// This is a shape check, not a checksum: it catches the mistake that actually
/// happens (right address, wrong chain) without pretending to fully validate
/// every chain's encoding.
pub fn validate_refund_address(network: SideshiftNetwork, address: &str) -> Result<(), String> {
    // Every message below names the network *after* a preposition rather than
    // after an article. "a {}" reads as "a Ethereum" for half the catalogue,
    // and the mismatch case can name two networks at once ("Ethereum or
    // Binance"), which no single article fits.
    let addr = address.trim();
    if addr.is_empty() {
        return Err(format!(
            "Enter your address on {} — a refund is returned there if the shift can't complete.",
            network.network_name()
        ));
    }

    let detected = SideshiftNetwork::detect_from_address(addr);
    if detected.is_empty() {
        return Err(format!(
            "That doesn't look like a valid address on {}.",
            network.network_name()
        ));
    }
    if !detected.contains(&network) {
        // The high-value case: a Bitcoin address pasted as a Tron refund
        // address, or an EVM address for a Solana deposit. Name both chains so
        // the user can see what they've mixed up.
        let detected_names = detected
            .iter()
            .map(|n| n.network_name())
            .collect::<Vec<_>>()
            .join(" or ");
        return Err(format!(
            "That address is on {detected_names}, but your refund must go back to {} — the \
             network you're depositing from. Funds refunded to the wrong network are lost.",
            network.network_name(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Flagged / held shifts ───────────────────────────────────────────

    #[test]
    fn a_held_shift_is_terminal_and_asks_the_user_to_act() {
        // The bug this guards: `review` falling into `Unknown`, which is
        // non-terminal, so the poller spins forever and the user watches a
        // spinner for a shift that will never move on its own.
        let review = ShiftStatusKind::from("review");
        assert_eq!(review, ShiftStatusKind::Review);
        assert!(
            review.is_terminal(),
            "a held shift will not advance by itself"
        );
        assert!(review.needs_user_action());
        assert!(review.guidance().contains("contact SideShift"));
    }

    #[test]
    fn sideshift_spells_refunds_both_ways() {
        // Their API has used both spellings; mapping only one leaves the other
        // as `Unknown`, which reads as non-terminal.
        assert_eq!(ShiftStatusKind::from("refunded"), ShiftStatusKind::Refunded);
        assert_eq!(ShiftStatusKind::from("refund"), ShiftStatusKind::Refunded);
        assert_eq!(
            ShiftStatusKind::from("refunding"),
            ShiftStatusKind::Refunding
        );
        assert!(ShiftStatusKind::Refunded.is_terminal());
        // Still in flight — don't stop polling.
        assert!(!ShiftStatusKind::Refunding.is_terminal());
    }

    #[test]
    fn in_flight_states_keep_polling_and_set_expectations() {
        for s in ["waiting", "pending", "processing", "settling"] {
            let kind = ShiftStatusKind::from(s);
            assert!(!kind.is_terminal(), "{} should keep polling", s);
            assert!(!kind.needs_user_action(), "{} needs no user action", s);
        }
        // On-chain settle means a confirmation wait; the copy must say so
        // rather than implying it's instant.
        assert!(ShiftStatusKind::Settling
            .guidance()
            .contains("confirmation"));
    }

    #[test]
    fn settled_copy_says_bitcoin_never_the_deposited_asset() {
        // Master invariant 8: the user receives BTC at the shift rate. Telling
        // them they "received USDT" would be false.
        let copy = ShiftStatusKind::Settled.guidance();
        assert!(copy.contains("bitcoin"));
        assert!(!copy.to_lowercase().contains("usdt"));
    }

    // ── Refund address validation ───────────────────────────────────────

    #[test]
    fn a_refund_address_must_match_the_origin_network() {
        // Tron deposit, Tron refund address: fine.
        assert!(validate_refund_address(
            SideshiftNetwork::Tron,
            "TJRyWwFs9wTFGZg3JbrVriFbNfCug5tDeC"
        )
        .is_ok());
        // EVM deposit, EVM refund address: fine.
        assert!(validate_refund_address(
            SideshiftNetwork::Ethereum,
            "0x71C7656EC7ab88b098defB751B7401B5f6d8976F"
        )
        .is_ok());
    }

    #[test]
    fn a_refund_address_on_the_wrong_chain_is_rejected_by_name() {
        // The mistake users actually make: they're receiving bitcoin, so they
        // paste an address from the chain they're depositing *to*, not from.
        // Refunds go back to the origin chain, so this loses the money.
        let err = validate_refund_address(
            SideshiftNetwork::Tron,
            "0x71C7656EC7ab88b098defB751B7401B5f6d8976F",
        )
        .unwrap_err();
        assert!(err.contains("Tron"), "must name the network we expected");
        assert!(err.contains("lost"));

        // And the inverse.
        assert!(validate_refund_address(
            SideshiftNetwork::Ethereum,
            "TJRyWwFs9wTFGZg3JbrVriFbNfCug5tDeC"
        )
        .is_err());
    }

    #[test]
    fn an_empty_or_unrecognisable_refund_address_is_rejected() {
        assert!(validate_refund_address(SideshiftNetwork::Tron, "").is_err());
        assert!(validate_refund_address(SideshiftNetwork::Tron, "   ").is_err());
        assert!(validate_refund_address(SideshiftNetwork::Solana, "not-an-address").is_err());
    }

    // ── Deposit catalogue ───────────────────────────────────────────────

    #[test]
    fn every_offered_deposit_is_one_we_can_refund() {
        // The invariant that makes the curated list a safety property rather
        // than a convenience: we must never offer a deposit whose refund
        // address we can't validate, or we've built a way to lose money.
        for option in deposit_options() {
            // A representative address for each network must validate against
            // that network — i.e. refund validation actually covers it.
            let sample = match option.network {
                SideshiftNetwork::Tron => "TJRyWwFs9wTFGZg3JbrVriFbNfCug5tDeC",
                SideshiftNetwork::Ethereum | SideshiftNetwork::Binance => {
                    "0x71C7656EC7ab88b098defB751B7401B5f6d8976F"
                }
                SideshiftNetwork::Solana => "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM",
                // A blech32 (`lq1`) confidential Liquid address.
                SideshiftNetwork::Liquid => {
                    "lq1qqwsq50k0h9y4rmwjr8t6dhtxcuyj8kn9h9lyqfaj2ple62rk3rgc46t6ck7mpstf7z6htg8p8vnac9c6k5xhntw5r0z3s7q8"
                }
            };
            assert!(
                validate_refund_address(option.network, sample).is_ok(),
                "no working refund validation for {}",
                option.label
            );
        }
    }

    #[test]
    fn liquid_usdt_is_offered_and_refund_validates_on_liquid() {
        // The Spark bridge accepts Liquid USDt in (settled as Spark BTC) — a
        // natural exit for users winding down a grandfathered Liquid wallet.
        let liquid_usdt = deposit_options()
            .iter()
            .find(|o| o.coin == "usdt" && o.network == SideshiftNetwork::Liquid)
            .expect("USDt on Liquid must be an offered deposit");
        assert_eq!(liquid_usdt.key(), "usdt:liquid");

        // A refund of a Liquid deposit goes back to a Liquid address. Cover the
        // three address shapes SideShift/Blockstream emit: blech32, confidential
        // (VJL), and unconfidential (Q, 34 chars).
        for addr in [
            "lq1qqwsq50k0h9y4rmwjr8t6dhtxcuyj8kn9h9lyqfaj2ple62rk3rgc46t6ck7mpstf7z6htg8p8vnac9c6k5xhntw5r0z3s7q8",
            "VJLBnap7fdedYbn8ez5c3Abz9nCP7oV6JGYUpvPthkbe4QC4XjNGaFN4jNRVBWvNJUnMkmVpB",
            "QLZKYZ7c8p9Np8xkU6Qk8Xz4rQ9k5m3nDe",
        ] {
            assert!(
                validate_refund_address(SideshiftNetwork::Liquid, addr).is_ok(),
                "Liquid refund address rejected: {}",
                addr
            );
        }

        // And a non-Liquid address as a Liquid refund is refused — the wrong
        // network loses the funds.
        assert!(validate_refund_address(
            SideshiftNetwork::Liquid,
            "0x71C7656EC7ab88b098defB751B7401B5f6d8976F"
        )
        .is_err());
    }

    #[test]
    fn deposit_options_round_trip_through_their_key() {
        // The key is what crosses a message boundary, so a mismatch would show
        // up as a silently unselectable option.
        for option in deposit_options() {
            assert_eq!(deposit_option_by_key(&option.key()), Some(*option));
        }
        assert_eq!(deposit_option_by_key("nope:nowhere"), None);
    }

    #[test]
    fn the_same_asset_on_different_chains_stays_distinct() {
        // USDT on Tron and USDT on Ethereum are different deposits with
        // different refund chains; collapsing them would be a funds-loss bug.
        let tron = deposit_option_by_key("usdt:tron").expect("usdt on tron");
        let eth = deposit_option_by_key("usdt:ethereum").expect("usdt on ethereum");
        assert_ne!(tron.network, eth.network);
        assert_ne!(tron.key(), eth.key());
    }

    #[test]
    fn a_completed_refund_is_not_described_as_still_in_progress() {
        // "is being returned" for a refund that already landed sends the user
        // looking for a transfer that has, in fact, arrived.
        assert!(ShiftStatusKind::Refunding.guidance().contains("is being"));
        assert!(ShiftStatusKind::Refunded.guidance().contains("has been"));
        assert!(!ShiftStatusKind::Refunded.guidance().contains("is being"));
    }

    /// Every message names the network after a preposition, never after an
    /// article — "a Ethereum address" is what you get otherwise, and the
    /// mismatch case can name two networks at once, which no article fits.
    #[test]
    fn refund_errors_stay_grammatical_for_every_network() {
        for option in deposit_options() {
            let net = option.network;
            for msg in [
                validate_refund_address(net, "").unwrap_err(),
                validate_refund_address(net, "definitely-not-an-address").unwrap_err(),
            ] {
                assert!(
                    !msg.contains(&format!("a {}", net.network_name())),
                    "ungrammatical article before the network name: {}",
                    msg
                );
            }
        }
    }

    #[test]
    fn a_multi_network_detection_reads_as_a_list_not_a_mangled_article() {
        // An EVM address detects as Ethereum *and* Binance. The old phrasing
        // joined them with " or a " and dropped the leading article entirely.
        let err = validate_refund_address(
            SideshiftNetwork::Solana,
            "0x71C7656EC7ab88b098defB751B7401B5f6d8976F",
        )
        .unwrap_err();
        assert!(err.contains("Ethereum or Binance"), "{}", err);
        assert!(err.contains("Solana"));
        assert!(err.contains("lost"));
    }

    #[test]
    fn an_ambiguous_evm_address_satisfies_either_evm_chain() {
        // `detect_from_address` can't tell Ethereum from BSC — same format. So
        // an EVM address must validate for both, rather than being rejected for
        // whichever one it wasn't guessed as first.
        let evm = "0x71C7656EC7ab88b098defB751B7401B5f6d8976F";
        assert!(validate_refund_address(SideshiftNetwork::Ethereum, evm).is_ok());
        assert!(validate_refund_address(SideshiftNetwork::Binance, evm).is_ok());
    }
}

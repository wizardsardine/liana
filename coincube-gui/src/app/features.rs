//! Network-aware feature availability — the single source of truth for
//! which wallet/marketplace features work on which Bitcoin network.
//!
//! On each network the launcher supports (mainnet, testnet, testnet4,
//! signet, regtest) only some features have a real backend. Rather than
//! hiding unsupported features, the nav renders them disabled / greyed
//! out with a hover popover whose text comes from [`Availability::reason`].
//!
//! Everything that needs to know "is feature X usable on network Y" asks
//! this module — the nav rails, the route guard, etc. — so the
//! matrix lives in exactly one place. See `plans/PLAN-network-feature-gating.md`.

use crate::app::menu::{MarketplaceSubMenu, Menu, P2PSubMenu};
#[cfg(test)]
use crate::chain::ChainIdExt;
use crate::chain::{ChainId, RuntimeSupport};

/// Whether a feature is usable on the current network, plus the human
/// reason to show when it isn't.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Availability {
    Available,
    /// Shown verbatim in the disabled item's popover.
    Unavailable {
        reason: String,
    },
}

impl Availability {
    pub fn is_available(&self) -> bool {
        matches!(self, Availability::Available)
    }

    /// The popover text, or `None` when the feature is available.
    pub fn reason(&self) -> Option<&str> {
        match self {
            Availability::Unavailable { reason } => Some(reason),
            Availability::Available => None,
        }
    }
}

/// Server-controlled availability of the Marketplace, sourced from
/// `GET /connect/features`. This is a second, independent dimension on top
/// of the per-network gate above: the network gate greys features out with
/// a "not on this network" reason, whereas these flags are a launch
/// kill-switch — when off, the whole surface is *hidden*, not greyed.
///
/// Fails **closed**. Every flag defaults to `false`, and callers hand this
/// its [`OFF`](Self::OFF) value before `/connect/features` has loaded (right
/// after sign-in), when the response omits the flags, or when the API is
/// unreachable. A launch build must never surface the untested money feature
/// because of a stale or missing API response. (Contrast the network gate,
/// which defaults to showing-but-disabled.)
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct MarketplaceServerFlags {
    /// Master switch. When `false` the entire Marketplace section is hidden
    /// and every route under it — including the otherwise always-reachable
    /// P2P `Settings` sub-route — is unreachable.
    pub marketplace_enabled: bool,
    /// Whether the server permits Buy/Sell. Only meaningful when
    /// `marketplace_enabled` is also `true` (see [`Self::buy_sell_on`]).
    pub buy_sell_enabled: bool,
    /// Whether the server permits P2P trading. Only meaningful when
    /// `marketplace_enabled` is also `true` (see [`Self::p2p_on`]).
    pub p2p_enabled: bool,
}

/// First-release product ceiling. Turning this on is a deliberate release
/// decision, independent of Connect defaults and per-account overrides.
const MARKETPLACE_ENABLED_FOR_RELEASE: bool = false;

impl MarketplaceServerFlags {
    /// Fail-closed value: everything off. The default state before features
    /// load, and the value used when the API is unreachable or silent.
    pub const OFF: Self = Self {
        marketplace_enabled: false,
        buy_sell_enabled: false,
        p2p_enabled: false,
    };

    /// Apply the local release ceiling before flags enter the cache used by
    /// navigation, route availability, and P2P subscription/action guards.
    pub fn for_release(self) -> Self {
        if MARKETPLACE_ENABLED_FOR_RELEASE {
            self
        } else {
            Self::OFF
        }
    }

    /// Whether the server permits Buy/Sell at all: the master switch AND the
    /// Buy/Sell flag. The single source of truth for "should Buy/Sell show".
    pub fn buy_sell_on(&self) -> bool {
        self.marketplace_enabled && self.buy_sell_enabled
    }

    /// Whether the server permits P2P at all: the master switch AND the P2P
    /// flag. The single source of truth for "should P2P show".
    pub fn p2p_on(&self) -> bool {
        self.marketplace_enabled && self.p2p_enabled
    }
}

/// Placeholder reason for a Marketplace route reached while the server has
/// the feature switched off. These routes are hidden from the nav, so this
/// only surfaces on a restored or deep-linked route (fail-closed backstop) —
/// deliberately generic, since "off at the server" is not a per-network state.
fn marketplace_off() -> Availability {
    Availability::Unavailable {
        reason: coincube_ui::locale::copy("The Marketplace isn't available right now.").to_string(),
    }
}

/// Whether the Duress Mode surface is shown at all — a server-flag dimension
/// with two inputs OR'd, and the Marketplace *hidden-not-greyed* kill-switch
/// semantics.
///
/// Two independent inputs, OR'd:
///
/// - `server_enabled` — the per-user `duressEnabled` flag from
///   `GET /connect/features`. Only meaningful on an *authenticated* call;
///   pre-login the API silently reports `false`, so this stays `false` until
///   features load for a signed-in user. Fails **closed** (absent / unloaded /
///   unreachable = `false`): the launch build ships duress dark, and a stale or
///   missing API response must never surface the untested setup wizard.
/// - `enrolled` — this account has completed duress enrollment. The client
///   mirror of the server's grandfather rule (master I4): an account enrolled
///   during beta keeps every duress surface — manage, trigger, clear — even
///   when prod later serves `duressEnabled: false` or Connect is unreachable.
///
/// The gated inputs both live in server / account state, not on disk:
/// enrollment is the durable half and the enrolled client learns
/// it (via account state) long before it could ever need the surface. So this
/// gate is recomputed live from the account panel and is **not** mirrored into
/// [`crate::app::cache::Cache`] or persisted to per-cube settings. An
/// un-enrolled user is fail-closed, exactly like Marketplace: hiding a setup
/// wizard costs them nothing.
///
/// This is only the launch/visibility half of the show-rule; the paid
/// `duress` entitlement (`ConnectAccountPanel::is_duress_entitled`) is a
/// separate, unchanged authority AND'd on top (see
/// `ConnectAccountPanel::show_duress`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct DuressGate {
    /// `duressEnabled` from `/connect/features` (authenticated). Defaults
    /// `false`: fails closed until the server grants it for a signed-in user.
    pub server_enabled: bool,
    /// This account has completed duress enrollment. Never cleared by a
    /// missing/`false` server flag — an enrolled account keeps duress forever.
    pub enrolled: bool,
}

impl DuressGate {
    /// Nothing granted, not enrolled — a fresh account before features load.
    /// The starting value everywhere.
    pub const OFF: Self = Self {
        server_enabled: false,
        enrolled: false,
    };

    /// The single source of truth for "should the duress surface exist" — the
    /// launch flag OR'd with enrollment. Consulted by the nav row, the route
    /// backstop, the duress view, and the enrollment-wizard backstop (each
    /// after AND'ing the `duress` entitlement).
    pub fn on(&self) -> bool {
        self.server_enabled || self.enrolled
    }
}

/// Display name for a chain. Shared by popover text and the settings
/// "Network:" row — deliberately exhaustive so a new variant is a compile
/// error rather than a silently wrong label. Keyed on the chain's identity:
/// Bitcoin Blake2b encodes like mainnet but is not "Mainnet".
pub(crate) fn net_label<C: Into<ChainId>>(chain: C) -> &'static str {
    match chain.into() {
        ChainId::Bitcoin => "Mainnet",
        ChainId::Testnet => "Testnet",
        ChainId::Testnet4 => "Testnet4",
        ChainId::Signet => "Signet",
        ChainId::Regtest => "Regtest",
        ChainId::BitcoinBlake2b => "Bitcoin Blake2b",
        ChainId::BitcoinBlake2bTestnet4 => "Bitcoin Blake2b Testnet4",
    }
}

/// Spark wallet. Backed only on mainnet and regtest — matches the SDK,
/// which rejects every other network (`breez_spark::config::SparkConfig`).
/// Never on Bitcoin Blake2b: the SDK would treat it as mainnet and settle
/// against the wrong chain.
pub fn spark<C: Into<ChainId>>(chain: C) -> Availability {
    match chain.into() {
        ChainId::Bitcoin | ChainId::Regtest => Availability::Available,
        other => unavailable("Spark", other),
    }
}

/// Buy/Sell (fiat on/off-ramp). Real fiat ↔ real BTC, so mainnet only —
/// and BTC only: a Bitcoin Blake2b Cube holds BTCB2, which no on-ramp sells.
pub fn buy_sell<C: Into<ChainId>>(chain: C) -> Availability {
    match chain.into() {
        ChainId::Bitcoin => Availability::Available,
        other => unavailable("Buy/Sell", other),
    }
}

/// P2P trading. Always available on mainnet; on a test network only when
/// a test Mostro coordinator is configured with a usable escrow rail (see
/// `view::p2p::config::MostroConfig::has_test_coordinator`, which resolves
/// the `has_test_coordinator` flag passed here).
pub fn p2p<C: Into<ChainId>>(chain: C, has_test_coordinator: bool) -> Availability {
    match chain.into() {
        ChainId::Bitcoin => Availability::Available,
        // No test coordinator lifts the gate on a Bitcoin Blake2b chain: the
        // escrow rail is a Spark wallet, and Spark is not offered there.
        other if other.is_blake2b() => unavailable("P2P trading", other),
        _ if has_test_coordinator => Availability::Available,
        // `has_test_coordinator` collapses two requirements (a configured test
        // coordinator *and* a connected Spark escrow wallet), so state both
        // rather than misattribute the block to a missing coordinator when the
        // coordinator is present but Spark is down.
        other => Availability::Unavailable {
            reason: coincube_ui::localized_format!(
                "P2P trading on {} needs a test Mostro coordinator and a connected Spark wallet.",
                "El intercambio P2P en {} necesita un coordinador Mostro de prueba y una cartera Spark conectada.",
                net_label(other)
            ),
        },
    }
}

/// Availability of whatever feature `menu` belongs to. Used to guard the
/// content area so a restored or deep-linked route onto a network-disabled
/// feature renders the shared "unavailable" placeholder instead of a live
/// panel (the rail items themselves are already greyed and inert). Routes
/// not tied to a gated feature are always available.
pub fn route_availability<C: Into<ChainId>>(
    menu: &Menu,
    chain: C,
    p2p_test_coordinator: bool,
    flags: MarketplaceServerFlags,
) -> Availability {
    let net = chain.into();
    match menu {
        Menu::Spark(_) => spark(net),
        // Buy/Sell: hidden entirely when the server switch is off, otherwise
        // subject to the per-network gate.
        Menu::Marketplace(MarketplaceSubMenu::BuySell) => {
            if flags.buy_sell_on() {
                buy_sell(net)
            } else {
                marketplace_off()
            }
        }
        // P2P Settings stays reachable even when *network* trading is gated,
        // so users can configure a coordinator to lift that gate — otherwise
        // it's a catch-22 (you'd need a working coordinator to reach the page
        // that adds one, and removing the last test node would lock you out).
        // But when the *server* has P2P off, there's nothing to configure, so
        // Settings is unreachable too — matching the hide-everything intent.
        Menu::Marketplace(MarketplaceSubMenu::P2P(P2PSubMenu::Settings)) => {
            if flags.p2p_on() {
                Availability::Available
            } else {
                marketplace_off()
            }
        }
        Menu::Marketplace(MarketplaceSubMenu::P2P(_)) => {
            if flags.p2p_on() {
                p2p(net, p2p_test_coordinator)
            } else {
                marketplace_off()
            }
        }
        _ => Availability::Available,
    }
}

fn unavailable(feature: &str, net: ChainId) -> Availability {
    Availability::Unavailable {
        reason: coincube_ui::localized_format!(
            "{} isn't available on {}.",
            "{} no está disponible en {}.",
            coincube_ui::locale::copy(feature),
            net_label(net)
        ),
    }
}

/// The server's Bitcoin Blake2b launch flag (`bitcoinBlake2bEnabled` on
/// `GET /connect/features`, wire name agreed with Connect PR 1).
///
/// This is a **capability signal from the server, not an availability
/// verdict**. It says Connect will accept BTCB2 keychains, keys and Cubes for
/// this account; it says nothing about whether *this build* can run one. The
/// verdict is [`bitcoin_blake2b`], which AND's this with
/// the explicit authenticated Connect capability. It does not grant generic
/// runtime, managed-node, migration or duress support.
///
/// Fails **closed**, like the Marketplace flags: absent, unloaded or an
/// unreachable API all read as `false`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct BitcoinBlake2bServerFlag {
    pub server_enabled: bool,
}

/// The local beta preference never grants a server capability on its own.
pub(crate) fn bitcoin_blake2b_enabled(server_enabled: bool, opted_in: bool) -> bool {
    server_enabled && opted_in
}

impl BitcoinBlake2bServerFlag {
    /// The value before `/connect/features` has answered, and when it can't.
    pub const OFF: Self = Self {
        server_enabled: false,
    };
}

/// Whether a Bitcoin Blake2b Cube may be created, opened or signed with in
/// this build for this account. The only place that question is answered.
///
/// Both the explicit Connect capability and the authenticated server flag
/// are required. Generic runtime support remains dormant. Installer, PIN and
/// Loader boundaries recheck the current flag before admission/decryption.
pub fn bitcoin_blake2b(flag: BitcoinBlake2bServerFlag) -> Availability {
    bitcoin_blake2b_with(
        crate::chain::authenticated_connect_support(ChainId::BitcoinBlake2b),
        flag,
    )
}

/// [`bitcoin_blake2b`] with the runtime support injected, so the full matrix
/// is testable independently of the narrower production capability.
fn bitcoin_blake2b_with(support: RuntimeSupport, flag: BitcoinBlake2bServerFlag) -> Availability {
    match support {
        RuntimeSupport::Dormant { reason } => Availability::Unavailable {
            reason: reason.to_string(),
        },
        RuntimeSupport::Supported if !flag.server_enabled => Availability::Unavailable {
            reason: "Bitcoin Blake2b isn't enabled for this account.".to_string(),
        },
        RuntimeSupport::Supported => Availability::Available,
    }
}

/// Everything a Cube must be for a Bitcoin Blake2b **claim** to start from it.
///
/// Deliberately one predicate rather than a condition spread over the Home
/// card and the Vault rail: the two surfaces must agree, and "the card was
/// shown" is not a permission the installer can check later.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClaimSourceCube {
    /// The chain the candidate source Cube is on. Only Bitcoin mainnet has
    /// coins a fork claim can be about.
    pub chain: ChainId,
    /// Whether it has a Vault. The descriptor *is* what a claim reuses, so a
    /// Cube without one has nothing to claim with.
    pub has_vault: bool,
    /// The account-scoped `/connect/features` grant, mirrored into the Cache.
    pub server_enabled: bool,
    /// Whether a Bitcoin Blake2b Cube already reuses this Cube's descriptor.
    /// A second target would watch the same addresses twice.
    pub already_claimed: bool,
}

/// Whether this Cube may have a Bitcoin Blake2b copy (Tools › Split coins →
/// Create copy). Both halves of the gate — [`bitcoin_blake2b`] for the
/// build/account, and the source Cube's own shape. Whether a copy already
/// exists is a separate question (`already_claimed`): the caller refuses a
/// second copy, which would watch the same addresses twice.
pub fn claim_blake2b(source: ClaimSourceCube) -> Availability {
    let account = bitcoin_blake2b(BitcoinBlake2bServerFlag {
        server_enabled: source.server_enabled,
    });
    if !account.is_available() {
        return account;
    }
    if source.chain != ChainId::Bitcoin {
        return Availability::Unavailable {
            reason: "Only a Bitcoin Cube can be claimed onto Bitcoin Blake2b.".to_string(),
        };
    }
    if !source.has_vault {
        return Availability::Unavailable {
            reason: "This Cube has no Vault to claim.".to_string(),
        };
    }
    Availability::Available
}

#[cfg(test)]
mod tests {
    use super::*;
    use coincube_core::miniscript::bitcoin::Network;

    const NETWORKS: [Network; 5] = [
        Network::Bitcoin,
        Network::Testnet,
        Network::Testnet4,
        Network::Signet,
        Network::Regtest,
    ];

    fn claim_source(chain: ChainId) -> ClaimSourceCube {
        ClaimSourceCube {
            chain,
            has_vault: true,
            server_enabled: true,
            already_claimed: false,
        }
    }

    /// The claim card's visibility matrix, in one place. Every `false` here is
    /// a surface the user must not be offered, and each has a distinct reason
    /// because the popover shows it verbatim.
    #[test]
    fn claim_visibility_matrix() {
        assert!(claim_blake2b(claim_source(ChainId::Bitcoin)).is_available());

        // Not a Bitcoin Cube: nothing on the fork chain to claim.
        for chain in [
            ChainId::Testnet,
            ChainId::Testnet4,
            ChainId::Signet,
            ChainId::Regtest,
            ChainId::BitcoinBlake2b,
            ChainId::BitcoinBlake2bTestnet4,
        ] {
            assert!(
                !claim_blake2b(claim_source(chain)).is_available(),
                "{:?} must not offer a claim",
                chain
            );
        }

        // No Vault: the descriptor is what a claim reuses.
        assert!(!claim_blake2b(ClaimSourceCube {
            has_vault: false,
            ..claim_source(ChainId::Bitcoin)
        })
        .is_available());

        // Already copied: still an eligible source; the caller refuses a
        // second copy, which would watch the same addresses twice.
        let claimed = ClaimSourceCube {
            already_claimed: true,
            ..claim_source(ChainId::Bitcoin)
        };
        assert!(claim_blake2b(claimed).is_available());

        // Account half absent — fails closed, and keeps the account-level
        // wording rather than a per-Cube one.
        let without_grant = claim_blake2b(ClaimSourceCube {
            server_enabled: false,
            ..claim_source(ChainId::Bitcoin)
        });
        assert!(!without_grant.is_available());
        assert_eq!(
            without_grant.reason(),
            bitcoin_blake2b(BitcoinBlake2bServerFlag::OFF).reason(),
            "the account gate answers for itself"
        );

        // Distinct reasons: the popover renders them verbatim.
        let reasons: std::collections::HashSet<String> = [
            claim_blake2b(claim_source(ChainId::Testnet)),
            claim_blake2b(ClaimSourceCube {
                has_vault: false,
                ..claim_source(ChainId::Bitcoin)
            }),
            claim_blake2b(ClaimSourceCube {
                server_enabled: false,
                ..claim_source(ChainId::Bitcoin)
            }),
        ]
        .iter()
        .filter_map(|a| a.reason().map(str::to_string))
        .collect();
        assert_eq!(reasons.len(), 3);
    }

    /// Regression guard for the §2 support matrix. If a decision changes,
    /// this is the one place to update alongside the matching `fn`.
    #[test]
    fn support_matrix() {
        // (network, spark, buy_sell, p2p_without_coordinator)
        let expected = [
            (Network::Bitcoin, true, true, true),
            (Network::Testnet, false, false, false),
            (Network::Testnet4, false, false, false),
            (Network::Signet, false, false, false),
            (Network::Regtest, true, false, false),
        ];

        for (net, spark_ok, buy_sell_ok, p2p_ok) in expected {
            assert_eq!(spark(net).is_available(), spark_ok, "spark on {}", net);
            assert_eq!(
                buy_sell(net).is_available(),
                buy_sell_ok,
                "buy_sell on {}",
                net
            );
            assert_eq!(
                p2p(net, false).is_available(),
                p2p_ok,
                "p2p (no coordinator) on {}",
                net
            );
        }
    }

    #[test]
    fn test_coordinator_enables_p2p_on_test_networks() {
        for net in NETWORKS {
            // With a test coordinator, P2P is available everywhere.
            assert!(
                p2p(net, true).is_available(),
                "p2p with coordinator on {}",
                net
            );
        }
    }

    /// Every marketplace flag combination that gates a route. Kept as its
    /// own table so the server dimension is legible next to the network one.
    const ALL_ON: MarketplaceServerFlags = MarketplaceServerFlags {
        marketplace_enabled: true,
        buy_sell_enabled: true,
        p2p_enabled: true,
    };

    fn buy_sell_route() -> Menu {
        Menu::Marketplace(MarketplaceSubMenu::BuySell)
    }
    fn p2p_route() -> Menu {
        Menu::Marketplace(MarketplaceSubMenu::P2P(P2PSubMenu::Overview))
    }
    fn p2p_settings_route() -> Menu {
        Menu::Marketplace(MarketplaceSubMenu::P2P(P2PSubMenu::Settings))
    }

    /// Marketplace/Spark route checks.
    fn avail<C: Into<ChainId>>(menu: &Menu, net: C, flags: MarketplaceServerFlags) -> bool {
        route_availability(menu, net, false, flags).is_available()
    }

    #[test]
    fn server_flags_default_and_off_are_fail_closed() {
        // Default derives to all-false, matching the explicit OFF constant.
        assert_eq!(
            MarketplaceServerFlags::default(),
            MarketplaceServerFlags::OFF
        );
        assert!(!MarketplaceServerFlags::OFF.buy_sell_on());
        assert!(!MarketplaceServerFlags::OFF.p2p_on());
    }

    #[test]
    fn first_release_rejects_true_server_flags_for_routes_and_actions() {
        let flags = ALL_ON.for_release();
        assert_eq!(flags, MarketplaceServerFlags::OFF);
        assert!(!flags.buy_sell_on());
        assert!(!flags.p2p_on());
        assert!(!avail(&buy_sell_route(), Network::Bitcoin, flags));
        assert!(!avail(&p2p_route(), Network::Bitcoin, flags));
        assert!(!avail(&p2p_settings_route(), Network::Bitcoin, flags));
    }

    #[test]
    fn sub_flags_require_the_master_switch() {
        // Sub-flags on but master off → still off (fail-closed).
        let master_off = MarketplaceServerFlags {
            marketplace_enabled: false,
            buy_sell_enabled: true,
            p2p_enabled: true,
        };
        assert!(!master_off.buy_sell_on());
        assert!(!master_off.p2p_on());
    }

    #[test]
    fn marketplace_off_hides_every_route_including_p2p_settings() {
        // Fail-closed: with the server switch off, no Marketplace route is
        // reachable on mainnet — not even P2P Settings, which is otherwise
        // always available.
        for flags in [
            MarketplaceServerFlags::OFF,
            MarketplaceServerFlags::default(),
        ] {
            assert!(!avail(&buy_sell_route(), Network::Bitcoin, flags));
            assert!(!avail(&p2p_route(), Network::Bitcoin, flags));
            assert!(!avail(&p2p_settings_route(), Network::Bitcoin, flags));
        }
    }

    #[test]
    fn buy_sell_on_p2p_off_leaves_only_buy_sell_reachable() {
        let flags = MarketplaceServerFlags {
            marketplace_enabled: true,
            buy_sell_enabled: true,
            p2p_enabled: false,
        };
        assert!(avail(&buy_sell_route(), Network::Bitcoin, flags));
        assert!(!avail(&p2p_route(), Network::Bitcoin, flags));
        assert!(!avail(&p2p_settings_route(), Network::Bitcoin, flags));
    }

    #[test]
    fn p2p_on_buy_sell_off_leaves_only_p2p_reachable() {
        let flags = MarketplaceServerFlags {
            marketplace_enabled: true,
            buy_sell_enabled: false,
            p2p_enabled: true,
        };
        assert!(!avail(&buy_sell_route(), Network::Bitcoin, flags));
        assert!(avail(&p2p_route(), Network::Bitcoin, flags));
        // Settings reachable so a coordinator can be configured.
        assert!(avail(&p2p_settings_route(), Network::Bitcoin, flags));
    }

    #[test]
    fn network_gate_still_applies_once_server_flags_are_on() {
        // Server says yes everywhere, but Buy/Sell + P2P have no backend on
        // signet, so the per-network gate still blocks them — while P2P
        // Settings stays reachable (configure a coordinator).
        assert!(!avail(&buy_sell_route(), Network::Signet, ALL_ON));
        assert!(!avail(&p2p_route(), Network::Signet, ALL_ON));
        assert!(avail(&p2p_settings_route(), Network::Signet, ALL_ON));
        // On mainnet everything is reachable once the flags are on.
        assert!(avail(&buy_sell_route(), Network::Bitcoin, ALL_ON));
        assert!(avail(&p2p_route(), Network::Bitcoin, ALL_ON));
    }

    #[test]
    fn non_marketplace_routes_ignore_server_flags() {
        // A Spark route is unaffected by marketplace flags (still network-gated).
        let spark_route = Menu::Spark(crate::app::menu::SparkSubMenu::Overview);
        assert!(avail(
            &spark_route,
            Network::Bitcoin,
            MarketplaceServerFlags::OFF
        ));
        assert!(!avail(&spark_route, Network::Signet, ALL_ON));
    }

    // ── Duress gate ─────────────────────────────────────────────────────
    //
    // The launch/visibility half of the duress show-rule (PLAN-feature-flags).
    // The load-bearing rows are the grandfather ones: an enrolled account keeps
    // the surface even when the server flag is off (master I4).

    #[test]
    fn duress_gate_off_is_fail_closed_and_matches_default() {
        assert!(!DuressGate::OFF.on());
        assert_eq!(DuressGate::default(), DuressGate::OFF);
    }

    #[test]
    fn duress_gate_on_truth_table() {
        // on() == server_enabled OR enrolled — the full 2×2.
        let cases = [
            (false, false, false), // fresh, un-enrolled, flag off → hidden
            (true, false, true),   // server granted the flag → shown
            (false, true, true),   // grandfathered: enrolled beats a flag-off
            (true, true, true),    // both → shown
        ];
        for (server_enabled, enrolled, expected) in cases {
            let gate = DuressGate {
                server_enabled,
                enrolled,
            };
            assert_eq!(
                gate.on(),
                expected,
                "server_enabled={server_enabled} enrolled={enrolled}"
            );
        }
    }

    #[test]
    fn unavailable_reasons_read_correctly() {
        assert_eq!(
            spark(Network::Testnet4).reason(),
            Some("Spark isn't available on Testnet4.")
        );
        assert_eq!(
            buy_sell(Network::Signet).reason(),
            Some("Buy/Sell isn't available on Signet.")
        );
        assert_eq!(
            p2p(Network::Testnet, false).reason(),
            Some("P2P trading on Testnet needs a test Mostro coordinator and a connected Spark wallet.")
        );
        assert_eq!(spark(Network::Bitcoin).reason(), None);
    }

    /// Pins every label, not just the ones a reason string happens to
    /// exercise. The settings "Network:" row used to keep its own copy of
    /// this match with an `_ => "Unknown"` arm, which is how Testnet4 ended
    /// up displayed as "Unknown" there.
    #[test]
    fn net_labels_are_exhaustive_and_stable() {
        let expected = [
            (Network::Bitcoin, "Mainnet"),
            (Network::Testnet, "Testnet"),
            (Network::Testnet4, "Testnet4"),
            (Network::Signet, "Signet"),
            (Network::Regtest, "Regtest"),
        ];

        for (net, label) in expected {
            assert_eq!(net_label(net), label, "label for {}", net);
        }

        // Every network the launcher offers has a label above — keeps this
        // table honest if `NETWORKS` grows.
        assert_eq!(expected.len(), NETWORKS.len());
        for net in NETWORKS {
            assert!(
                expected.iter().any(|(n, _)| *n == net),
                "{} missing from the label table",
                net
            );
        }
    }
    // ── Bitcoin Blake2b (BTCB2 plan PR 2, dormant slice) ───────────────────

    #[test]
    fn every_network_feature_is_unavailable_on_both_blake2b_chains() {
        for chain in [ChainId::BitcoinBlake2b, ChainId::BitcoinBlake2bTestnet4] {
            assert!(!spark(chain).is_available(), "{:?}", chain);
            assert!(!buy_sell(chain).is_available(), "{:?}", chain);
            // Even a configured test coordinator does not lift P2P here.
            assert!(!p2p(chain, true).is_available(), "{:?}", chain);
            assert!(!p2p(chain, false).is_available(), "{:?}", chain);
            // The reasons name the chain, not "Mainnet".
            for reason in [
                spark(chain).reason().unwrap().to_string(),
                buy_sell(chain).reason().unwrap().to_string(),
                p2p(chain, true).reason().unwrap().to_string(),
            ] {
                assert!(reason.contains("Bitcoin Blake2b"), "{}", reason);
                assert!(!reason.contains("Mainnet"), "{}", reason);
            }
        }
        // …and the encoding twin is untouched by the new arms.
        assert!(spark(ChainId::Bitcoin).is_available());
        assert!(buy_sell(ChainId::Bitcoin).is_available());
    }

    #[test]
    fn blake2b_routes_are_gated_even_with_every_server_switch_on() {
        for chain in [ChainId::BitcoinBlake2b, ChainId::BitcoinBlake2bTestnet4] {
            let spark_route = Menu::Spark(crate::app::menu::SparkSubMenu::Overview);
            assert!(!avail(&spark_route, chain, ALL_ON), "{:?}", chain);
            assert!(!avail(&buy_sell_route(), chain, ALL_ON), "{:?}", chain);
            assert!(!avail(&p2p_route(), chain, ALL_ON), "{:?}", chain);
        }
    }

    #[test]
    fn net_label_names_the_blake2b_chains_without_calling_them_mainnet() {
        assert_eq!(net_label(ChainId::BitcoinBlake2b), "Bitcoin Blake2b");
        assert_eq!(
            net_label(ChainId::BitcoinBlake2bTestnet4),
            "Bitcoin Blake2b Testnet4"
        );
        assert_eq!(net_label(ChainId::Bitcoin), "Mainnet");
        assert_eq!(net_label(Network::Bitcoin), "Mainnet");
    }

    #[test]
    fn the_server_flag_is_a_capability_signal_not_an_availability_verdict() {
        const ON: BitcoinBlake2bServerFlag = BitcoinBlake2bServerFlag {
            server_enabled: true,
        };
        // Explicit authenticated Connect capability is available; generic
        // startup remains dormant and cannot consume this account flag.
        let verdict = bitcoin_blake2b(ON);
        assert!(verdict.is_available());
        assert!(!ChainId::BitcoinBlake2b.runtime_support().is_supported());
        assert!(!bitcoin_blake2b(BitcoinBlake2bServerFlag::OFF).is_available());
        assert!(!bitcoin_blake2b(BitcoinBlake2bServerFlag::default()).is_available());

        // The full matrix, with the runtime injected: only Supported AND flag
        // on is Available; a supported runtime with the flag off is refused
        // with the account reason, not the dormant one.
        let dormant = RuntimeSupport::Dormant {
            reason: crate::chain::BTCB2_DORMANT_REASON,
        };
        assert!(!bitcoin_blake2b_with(dormant, ON).is_available());
        assert!(!bitcoin_blake2b_with(dormant, BitcoinBlake2bServerFlag::OFF).is_available());
        let off = bitcoin_blake2b_with(RuntimeSupport::Supported, BitcoinBlake2bServerFlag::OFF);
        assert_eq!(
            off.reason(),
            Some("Bitcoin Blake2b isn't enabled for this account.")
        );
        assert!(bitcoin_blake2b_with(RuntimeSupport::Supported, ON).is_available());
    }
}

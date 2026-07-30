//! Network-aware feature availability — the single source of truth for
//! which wallet/marketplace features work on which Bitcoin network.
//!
//! On each network the launcher supports (mainnet, testnet, testnet4,
//! signet, regtest) only some features have a real backend. Rather than
//! hiding unsupported features, the nav renders them disabled / greyed
//! out with a hover popover whose text comes from [`Availability::reason`].
//!
//! Everything that needs to know "is feature X usable on network Y" asks
//! this module — the nav rails, the Liquid SDK loader, etc. — so the
//! matrix lives in exactly one place. See `plans/PLAN-network-feature-gating.md`.

use crate::app::menu::{MarketplaceSubMenu, Menu, P2PSubMenu};
use coincube_core::miniscript::bitcoin::Network;

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

impl MarketplaceServerFlags {
    /// Fail-closed value: everything off. The default state before features
    /// load, and the value used when the API is unreachable or silent.
    pub const OFF: Self = Self {
        marketplace_enabled: false,
        buy_sell_enabled: false,
        p2p_enabled: false,
    };

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
        reason: "The Marketplace isn't available right now.".to_string(),
    }
}

/// Whether the Liquid wallet is shown at all. This is the sunset gate: Liquid
/// is being wound down, so new installs don't get it, but anyone who already
/// has a Liquid wallet keeps full access to it forever.
///
/// Two independent inputs, OR'd — and note this deliberately does **not**
/// behave like [`MarketplaceServerFlags`]:
///
/// - `server_enabled` — the account-scoped `liquidEnabled` grandfather flag
///   from `GET /connect/features`. Only meaningful on an *authenticated* call;
///   pre-login the API silently reports `false`, so this stays `false` until
///   features load for a signed-in user.
/// - `local_state_exists` — a Liquid wallet has already been initialized on
///   this machine (see [`crate::app::breez_liquid::local_state_exists`]).
///
/// **Fails open on the local dimension, by design.** Marketplace is a launch
/// kill-switch and fails closed: hiding it costs a user nothing. Liquid holds
/// *funds*. If Connect is unreachable, the token expired, or the user never
/// made a Connect account at all, an existing Liquid wallet must still show —
/// otherwise the gate strands real L-BTC/L-USDt behind a network outage. So
/// local state alone is sufficient to surface the wallet, and the server flag
/// only ever *adds* visibility (for a fresh install the operator has
/// grandfathered).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct LiquidGate {
    /// `liquidEnabled` from `/connect/features` (authenticated). Defaults
    /// `false`: a fresh install gets no Liquid unless the server grants it.
    pub server_enabled: bool,
    /// A Liquid wallet has been initialized on this machine. Never cleared by
    /// an API response — funds access must not depend on Connect.
    pub local_state_exists: bool,
}

impl LiquidGate {
    /// Nothing granted, nothing on disk — a fresh install before features
    /// load. The starting value everywhere.
    pub const HIDDEN: Self = Self {
        server_enabled: false,
        local_state_exists: false,
    };

    /// The single source of truth for "should the Liquid surface exist".
    /// Consulted by the nav rails, the route guard, global home, and the
    /// wallet-creation entry points.
    pub fn show(&self) -> bool {
        self.server_enabled || self.local_state_exists
    }
}

/// Reason shown for a Liquid route reached while the wallet is gated off —
/// a restored or deep-linked route, since the nav hides Liquid entirely in
/// this state. Not a per-network message: Liquid is sunset for new wallets.
fn liquid_sunset() -> Availability {
    Availability::Unavailable {
        reason: "The Liquid wallet isn't available on this account.".to_string(),
    }
}

/// Whether the Duress Mode surface is shown at all — the third server-flag
/// dimension, structurally the [`LiquidGate`] shape (two inputs OR'd) but with
/// the Marketplace *hidden-not-greyed* kill-switch semantics.
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
/// Unlike [`LiquidGate`], the gated inputs both live in server / account state,
/// not on disk: enrollment is the durable half and the enrolled client learns
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

/// Display name for a network, used in popover text.
fn net_label(n: Network) -> &'static str {
    match n {
        Network::Bitcoin => "Mainnet",
        Network::Testnet => "Testnet",
        Network::Testnet4 => "Testnet4",
        Network::Signet => "Signet",
        Network::Regtest => "Regtest",
    }
}

/// Spark wallet. Backed only on mainnet and regtest — matches the SDK,
/// which rejects every other network (`breez_spark::config::SparkConfig`).
pub fn spark(net: Network) -> Availability {
    match net {
        Network::Bitcoin | Network::Regtest => Availability::Available,
        other => unavailable("Spark", other),
    }
}

/// Liquid wallet. Mainnet only. The Breez Liquid SDK (0.12.2) connects
/// solely on `LiquidNetwork::Mainnet` and `Regtest` — it hard-rejects
/// `LiquidNetwork::Testnet` at `connect_with_signer`, which is what
/// testnet *and* signet map to. Regtest would point Breez at a localhost
/// Esplora normal users don't run. So mainnet is the only network with a
/// usable Liquid backend.
pub fn liquid(net: Network) -> Availability {
    match net {
        Network::Bitcoin => Availability::Available,
        other => unavailable("Liquid", other),
    }
}

/// Buy/Sell (fiat on/off-ramp). Real fiat ↔ real BTC, so mainnet only.
pub fn buy_sell(net: Network) -> Availability {
    match net {
        Network::Bitcoin => Availability::Available,
        other => unavailable("Buy/Sell", other),
    }
}

/// P2P trading. Always available on mainnet; on a test network only when
/// a test Mostro coordinator is configured with a usable escrow rail (see
/// `view::p2p::config::MostroConfig::has_test_coordinator`, which resolves
/// the `has_test_coordinator` flag passed here).
pub fn p2p(net: Network, has_test_coordinator: bool) -> Availability {
    match net {
        Network::Bitcoin => Availability::Available,
        _ if has_test_coordinator => Availability::Available,
        // `has_test_coordinator` collapses two requirements (a configured test
        // coordinator *and* a connected Spark escrow wallet), so state both
        // rather than misattribute the block to a missing coordinator when the
        // coordinator is present but Spark is down.
        other => Availability::Unavailable {
            reason: format!(
                "P2P trading on {} needs a test Mostro coordinator and a connected Spark wallet.",
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
pub fn route_availability(
    menu: &Menu,
    net: Network,
    p2p_test_coordinator: bool,
    flags: MarketplaceServerFlags,
    liquid_gate: LiquidGate,
) -> Availability {
    match menu {
        Menu::Spark(_) => spark(net),
        // Sunset gate first, then the per-network gate — a gated-off Liquid
        // reads as "not on this account", not "not on this network".
        Menu::Liquid(_) => {
            if liquid_gate.show() {
                liquid(net)
            } else {
                liquid_sunset()
            }
        }
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

fn unavailable(feature: &str, net: Network) -> Availability {
    Availability::Unavailable {
        reason: format!("{} isn't available on {}.", feature, net_label(net)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NETWORKS: [Network; 5] = [
        Network::Bitcoin,
        Network::Testnet,
        Network::Testnet4,
        Network::Signet,
        Network::Regtest,
    ];

    /// Regression guard for the §2 support matrix. If a decision changes,
    /// this is the one place to update alongside the matching `fn`.
    #[test]
    fn support_matrix() {
        // (network, spark, liquid, buy_sell, p2p_without_coordinator)
        let expected = [
            (Network::Bitcoin, true, true, true, true),
            (Network::Testnet, false, false, false, false),
            (Network::Testnet4, false, false, false, false),
            (Network::Signet, false, false, false, false),
            (Network::Regtest, true, false, false, false),
        ];

        for (net, spark_ok, liquid_ok, buy_sell_ok, p2p_ok) in expected {
            assert_eq!(spark(net).is_available(), spark_ok, "spark on {}", net);
            assert_eq!(liquid(net).is_available(), liquid_ok, "liquid on {}", net);
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

    /// Marketplace/Spark route checks; Liquid is granted here so these stay
    /// about the flag under test. Liquid's own gate has its own tests below.
    fn avail(menu: &Menu, net: Network, flags: MarketplaceServerFlags) -> bool {
        route_availability(menu, net, false, flags, LIQUID_GRANTED).is_available()
    }

    const LIQUID_GRANTED: LiquidGate = LiquidGate {
        server_enabled: true,
        local_state_exists: false,
    };
    const LIQUID_LOCAL_ONLY: LiquidGate = LiquidGate {
        server_enabled: false,
        local_state_exists: true,
    };

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

    // ── Liquid sunset gate ──────────────────────────────────────────────
    //
    // The four scenarios from PLAN-liquid-sunset PR 1. The load-bearing one
    // is the last: unlike Marketplace, an unreachable API must never hide a
    // funded wallet.

    #[test]
    fn fresh_install_with_flag_off_hides_liquid() {
        assert!(!LiquidGate::HIDDEN.show());
        assert_eq!(LiquidGate::default(), LiquidGate::HIDDEN);
        assert!(!avail_liquid(Network::Bitcoin, LiquidGate::HIDDEN));
    }

    #[test]
    fn server_flag_grants_liquid_on_a_fresh_install() {
        assert!(LIQUID_GRANTED.show());
        assert!(avail_liquid(Network::Bitcoin, LIQUID_GRANTED));
    }

    #[test]
    fn local_wallet_shows_liquid_even_with_the_flag_off() {
        // The grandfather case: the server says no (or has never been asked),
        // but this machine already has a Liquid wallet. Funds win.
        assert!(LIQUID_LOCAL_ONLY.show());
        assert!(avail_liquid(Network::Bitcoin, LIQUID_LOCAL_ONLY));
    }

    #[test]
    fn unreachable_api_still_shows_a_funded_local_wallet() {
        // An unreachable/silent API reads as `server_enabled: false` — the
        // same value as a real "no". Local state must still surface the
        // wallet, or a network outage strands the user's L-BTC. This is the
        // deliberate divergence from `MarketplaceServerFlags`' fail-closed
        // behavior; if someone "fixes" this to fail closed, that's a bug.
        let api_down = LiquidGate {
            server_enabled: false,
            local_state_exists: true,
        };
        assert!(api_down.show());
    }

    #[test]
    fn network_gate_still_applies_to_a_granted_liquid_wallet() {
        // Being grandfathered in doesn't conjure a Liquid backend on signet.
        assert!(!avail_liquid(Network::Signet, LIQUID_GRANTED));
        assert!(!avail_liquid(Network::Regtest, LIQUID_LOCAL_ONLY));
    }

    #[test]
    fn gated_off_liquid_route_reads_as_account_scoped_not_network_scoped() {
        assert_eq!(
            route_availability(
                &Menu::Liquid(crate::app::menu::LiquidSubMenu::Overview),
                Network::Bitcoin,
                false,
                MarketplaceServerFlags::OFF,
                LiquidGate::HIDDEN,
            )
            .reason(),
            Some("The Liquid wallet isn't available on this account.")
        );
    }

    fn avail_liquid(net: Network, gate: LiquidGate) -> bool {
        route_availability(
            &Menu::Liquid(crate::app::menu::LiquidSubMenu::Overview),
            net,
            false,
            MarketplaceServerFlags::OFF,
            gate,
        )
        .is_available()
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
            liquid(Network::Regtest).reason(),
            Some("Liquid isn't available on Regtest.")
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
}

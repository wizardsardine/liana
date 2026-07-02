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
) -> Availability {
    match menu {
        Menu::Spark(_) => spark(net),
        Menu::Liquid(_) => liquid(net),
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

    fn avail(menu: &Menu, net: Network, flags: MarketplaceServerFlags) -> bool {
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

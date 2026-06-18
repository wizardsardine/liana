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
pub fn route_availability(menu: &Menu, net: Network, p2p_test_coordinator: bool) -> Availability {
    match menu {
        Menu::Spark(_) => spark(net),
        Menu::Liquid(_) => liquid(net),
        Menu::Marketplace(MarketplaceSubMenu::BuySell) => buy_sell(net),
        // P2P Settings stays reachable even when trading is gated, so users
        // can configure a coordinator to lift the gate — otherwise it's a
        // catch-22 (you'd need a working coordinator to reach the page that
        // adds one, and removing the last test node would lock you out).
        Menu::Marketplace(MarketplaceSubMenu::P2P(P2PSubMenu::Settings)) => Availability::Available,
        Menu::Marketplace(MarketplaceSubMenu::P2P(_)) => p2p(net, p2p_test_coordinator),
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

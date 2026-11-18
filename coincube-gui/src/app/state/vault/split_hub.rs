//! Tools › Split coins: a checklist of what is still on both chains, this
//! chain's split key, and the Bitcoin Blake2b copy (Tenshu canvas v157,
//! `P-Advanced` split section). Splitting itself happens in Coin control's
//! Split coins overlay; every action here leads there or to the copy wizard.

use std::sync::Arc;

use coincube_core::miniscript::bitcoin::Amount;
use coincube_ui::locale::copy as localized_copy;
use coincube_ui::{
    component::{amount::format_u64_as_string, button, text::*},
    localized_format, theme,
    widget::{Column, Container, Element, Row},
};
use iced::{Alignment, Length, Task};

use crate::{
    app::{
        cache::Cache,
        menu::Menu,
        message::Message,
        state::State,
        view::{self, Message as ViewMessage},
        wallet::Wallet,
    },
    chain::ChainId,
    daemon::Daemon,
    services::{chain_status::ChainStatus, split_coins},
};

/// One checklist line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HubItem {
    pub title: String,
    pub detail: String,
    pub state: ItemState,
    /// The action button, if any: its label and what it sends.
    pub action: Option<(&'static str, HubAction)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemState {
    Done,
    ToDo,
    Optional,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HubAction {
    /// Open Coin control's Split coins overlay.
    Split,
    /// Start the Bitcoin Blake2b copy wizard.
    CreateCopy,
}

fn chain_name(chain: ChainId) -> &'static str {
    if chain.is_blake2b() {
        "Bitcoin Blake2b"
    } else {
        "Bitcoin"
    }
}

fn sats(amount: Amount) -> String {
    format!("{} sats", format_u64_as_string(amount.to_sat(), ","))
}

/// The facts the checklist is built from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HubFacts {
    pub here: ChainId,
    pub both_chains: usize,
    pub both_total: Amount,
    pub splitting: usize,
    /// Unspent coins with no answer yet: not checked, still checking, a
    /// check that failed, or a signature split nothing has confirmed. Only
    /// positive evidence makes a coin done.
    pub unverified: usize,
    pub key: Option<Amount>,
    pub signature_available: bool,
    pub copy_exists: bool,
    /// Whether the copy can be created from here: a Vault through the copy
    /// wizard, an imported Vault as its paired imported Cube.
    pub can_create_copy: bool,
}

/// The checklist, worded in the window's language: it is built while the
/// panel renders.
pub fn items(facts: HubFacts) -> Vec<HubItem> {
    let here = facts.here;
    let coins = HubItem {
        title: localized_copy("Coins on both chains").into(),
        detail: if facts.both_chains == 1 {
            localized_format!(
                "1 coin · {}. Split them from either chain; one side is enough.",
                "1 moneda · {}. Sepárala desde cualquiera de las dos cadenas; basta con un lado.",
                sats(facts.both_total)
            )
        } else if facts.both_chains > 1 {
            localized_format!(
                "{} coins · {}. Split them from either chain; one side is enough.",
                "{} monedas · {}. Sepáralas desde cualquiera de las dos cadenas; basta con un lado.",
                facts.both_chains,
                sats(facts.both_total)
            )
        } else if facts.splitting > 0 {
            localized_copy("Splitting. The coin rows show the confirmations.").into()
        } else if facts.unverified == 1 {
            localized_format!(
                "1 coin not verified yet. Coin control shows each coin's status.",
                "1 moneda aún sin verificar. El Control de monedas muestra el estado de cada moneda."
            )
        } else if facts.unverified > 1 {
            localized_format!(
                "{} coins not verified yet. Coin control shows each coin's status.",
                "{} monedas aún sin verificar. El Control de monedas muestra el estado de cada moneda.",
                facts.unverified
            )
        } else {
            localized_copy("None. Every coin is split.").into()
        },
        state: if facts.both_chains > 0 || facts.splitting > 0 || facts.unverified > 0 {
            ItemState::ToDo
        } else {
            ItemState::Done
        },
        action: (facts.both_chains > 0).then_some(("Split coins", HubAction::Split)),
    };
    let signature = here.is_blake2b() && facts.signature_available;
    let key = HubItem {
        title: localized_format!("Split key on {}", "Clave dividida en {}", chain_name(here)),
        detail: match facts.key {
            Some(amount) => localized_format!(
                "{} · found only on {}. It comes back with every split, so one is enough.",
                "{} · solo está en {}. Vuelve con cada separación, así que basta con una.",
                sats(amount),
                chain_name(here)
            ),
            None if signature => localized_copy(
                "Not needed: this Vault can split with a replay-protected signature.",
            )
            .into(),
            None => localized_format!(
                "Needed to split from {}. The Split coins window shows where to send one.",
                "Hace falta para separar desde {}. La ventana Separar monedas muestra adónde \
                 enviar una.",
                chain_name(here)
            ),
        },
        state: if facts.key.is_some() || signature {
            ItemState::Done
        } else {
            ItemState::ToDo
        },
        action: (facts.key.is_none() && !signature)
            .then_some(("Get a split key", HubAction::Split)),
    };
    if here.is_blake2b() {
        vec![
            coins,
            HubItem {
                title: localized_copy("Replay-protected signature").into(),
                detail: if facts.signature_available {
                    localized_copy("Available: this Vault can sign it. No split key needed.").into()
                } else {
                    localized_copy("Not available: this Vault's signers can't make this signature. Use a split key.").into()
                },
                state: if facts.signature_available {
                    ItemState::Done
                } else {
                    ItemState::Optional
                },
                action: None,
            },
            key,
            HubItem {
                title: localized_copy("Or split from Bitcoin").into(),
                detail: localized_copy(
                    "A split in the Bitcoin Cube separates these coins here too.",
                )
                .into(),
                state: ItemState::Optional,
                action: None,
            },
        ]
    } else {
        vec![
            coins,
            key,
            HubItem {
                title: localized_copy("Bitcoin Blake2b copy").into(),
                detail: if facts.copy_exists {
                    localized_copy("Created. Open it from the Cube switcher.").into()
                } else {
                    localized_copy("Optional. Needed to spend your Bitcoin Blake2b coins, or to split them from that side.").into()
                },
                state: if facts.copy_exists {
                    ItemState::Done
                } else {
                    ItemState::Optional
                },
                action: (!facts.copy_exists && facts.can_create_copy)
                    .then_some(("Create copy", HubAction::CreateCopy)),
            },
        ]
    }
}

pub fn hub_view<'a>(here: ChainId, items: Vec<HubItem>) -> Element<'a, ViewMessage> {
    let intro = localized_format!(
        "Coins you held before the fork exist on both Bitcoin and Bitcoin Blake2b. Until one of them is spent in a way the other chain rejects, a transaction on one chain can be copied to the other and move both. Splitting from either chain is enough; you can also do both. This Cube is on {}.",
        "Las monedas que tenías antes de la bifurcación existen tanto en Bitcoin como en Bitcoin Blake2b. Hasta que una de ellas se gaste de una forma que la otra cadena rechace, una transacción en una cadena puede copiarse a la otra y mover ambas. Basta con separar desde cualquiera de las dos cadenas; también puedes hacerlo en ambas. Este Cube está en {}.",
        chain_name(here)
    );
    let list = items
        .into_iter()
        .fold(Column::new().spacing(12), |list, item| {
            let mark = match item.state {
                ItemState::Done => p1_bold("✓").style(theme::text::success),
                ItemState::ToDo => p1_bold("•").style(theme::text::warning),
                ItemState::Optional => p1_bold("○").style(theme::text::secondary),
            };
            let action = item.action.map(|(label, action)| {
                button::secondary(None, localized_copy(label)).on_press(match action {
                    HubAction::Split => ViewMessage::SplitFirst,
                    HubAction::CreateCopy => ViewMessage::CreateBlake2bCopy,
                })
            });
            list.push(
                Container::new(
                    Row::new()
                        .spacing(14)
                        .align_y(Alignment::Center)
                        .push(Container::new(mark).width(Length::Fixed(20.0)))
                        .push(
                            Column::new()
                                .spacing(4)
                                .width(Length::Fill)
                                .push(p1_bold(item.title))
                                .push(p2_regular(item.detail).style(theme::text::secondary)),
                        )
                        .push(action),
                )
                .padding(16)
                .style(theme::card::simple)
                .width(Length::Fill),
            )
        });
    Column::new()
        .spacing(20)
        .push(h3(localized_copy("Split coins")).bold())
        .push(p1_regular(intro).style(theme::text::secondary))
        .push(list)
        .into()
}

/// Tools › Split coins. Stateless: everything comes from the cache.
pub struct SplitHubPanel {
    wallet: Arc<Wallet>,
}

impl SplitHubPanel {
    pub fn new(wallet: Arc<Wallet>) -> Self {
        Self { wallet }
    }

    pub fn facts(&self, cache: &Cache) -> HubFacts {
        let both: Vec<_> = cache
            .coins()
            .iter()
            .filter(|coin| coin.spend_info.is_none() && coin.block_height.is_some())
            .filter(|coin| cache.chain_status(&coin.outpoint).needs_split())
            .collect();
        let unverified = cache
            .coins()
            .iter()
            .filter(|coin| coin.spend_info.is_none())
            .filter(|coin| {
                matches!(
                    cache.chain_status(&coin.outpoint),
                    ChainStatus::NotChecked
                        | ChainStatus::NotSeen
                        | ChainStatus::CouldntVerify
                        | ChainStatus::SplitUnconfirmed
                )
            })
            .count();
        HubFacts {
            here: self.wallet.chain,
            both_chains: both.len(),
            both_total: both.iter().map(|coin| coin.amount).sum(),
            splitting: cache.splitting.len(),
            unverified,
            key: cache.split_keys().first().map(|(coin, _)| coin.amount),
            signature_available: split_coins::signature_available(&self.wallet),
            copy_exists: cache.btcb2_already_claimed,
            can_create_copy: self.wallet.supports_inheritance() || self.wallet.is_imported(),
        }
    }
}

impl State for SplitHubPanel {
    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, ViewMessage> {
        let facts = self.facts(cache);
        view::dashboard(menu, cache, hub_view(facts.here, items(facts)))
    }

    fn update(
        &mut self,
        _daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        _cache: &Cache,
        _message: Message,
    ) -> Task<Message> {
        Task::none()
    }
}

impl From<SplitHubPanel> for Box<dyn State> {
    fn from(s: SplitHubPanel) -> Box<dyn State> {
        Box::new(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> HubFacts {
        HubFacts {
            here: ChainId::Bitcoin,
            both_chains: 3,
            both_total: Amount::from_sat(1_200_000),
            splitting: 0,
            unverified: 0,
            key: None,
            signature_available: false,
            copy_exists: false,
            can_create_copy: true,
        }
    }

    #[test]
    fn bitcoin_hub_lists_coins_key_and_optional_copy() {
        let items = items(facts());
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].action, Some(("Split coins", HubAction::Split)));
        assert_eq!(items[0].state, ItemState::ToDo);
        assert_eq!(items[1].action, Some(("Get a split key", HubAction::Split)));
        assert_eq!(items[2].title, "Bitcoin Blake2b copy");
        assert_eq!(
            items[2].state,
            ItemState::Optional,
            "the copy is never required on Bitcoin"
        );
        assert_eq!(
            items[2].action,
            Some(("Create copy", HubAction::CreateCopy))
        );
    }

    #[test]
    fn blake2b_hub_offers_the_signature_instead_of_a_key() {
        let items = items(HubFacts {
            here: ChainId::BitcoinBlake2b,
            signature_available: true,
            ..facts()
        });
        assert_eq!(items.len(), 4);
        assert_eq!(items[1].title, "Replay-protected signature");
        assert_eq!(items[1].state, ItemState::Done);
        assert_eq!(items[2].state, ItemState::Done, "no key needed");
        assert_eq!(items[2].action, None);
        assert_eq!(items[3].title, "Or split from Bitcoin");
    }

    #[test]
    fn nothing_left_reads_done() {
        let items = items(HubFacts {
            both_chains: 0,
            both_total: Amount::ZERO,
            key: Some(Amount::from_sat(30_000)),
            copy_exists: true,
            ..facts()
        });
        assert!(items.iter().all(|item| item.state == ItemState::Done));
        assert!(items.iter().all(|item| item.action.is_none()));
    }

    /// No both-chain coins is not done by itself: coins without an answer
    /// and splits still confirming keep the item open.
    #[test]
    fn coins_are_done_only_on_positive_evidence() {
        let done = HubFacts {
            both_chains: 0,
            both_total: Amount::ZERO,
            ..facts()
        };
        assert_eq!(items(done)[0].state, ItemState::Done);
        let unverified = items(HubFacts {
            unverified: 2,
            ..done
        });
        assert_eq!(unverified[0].state, ItemState::ToDo);
        assert!(unverified[0].detail.starts_with("2 coins not verified yet"));
        assert_eq!(unverified[0].action, None);
        let splitting = items(HubFacts {
            splitting: 1,
            ..done
        });
        assert_eq!(splitting[0].state, ItemState::ToDo);
        assert_eq!(splitting[0].action, None);
    }

    /// A signature split past its confirmations is "split but unconfirmed":
    /// the hub must keep Coins on both chains open. A spent copy of that
    /// flag, a coin verified on this chain, and a coin still on both chains
    /// must not be counted as that same gap.
    #[test]
    fn a_signature_split_stays_unverified_on_the_hub() {
        use crate::{daemon::model::Coin, services::entangled::Entanglement};
        use coincube_core::{
            descriptors::CoincubeDescriptor,
            miniscript::bitcoin::{bip32::ChildNumber, Address, OutPoint, Txid},
        };
        use coincubed::commands::LCSpendInfo;
        use std::{
            str::FromStr,
            time::{Duration, Instant},
        };

        const DESC: &str = "wsh(or_d(pk([f5acc2fd]tpubD6NzVbkrYhZ4YgUx2ZLNt2rLYAMTdYysCRzKoLu2BeSHKvzqPaBDvf17GeBPnExUVPkuBpx4kniP964e2MxyzzazcXLptxLXModSVCVEV1T/<0;1>/*),and_v(v:pkh([8a64f2a9]tpubD6NzVbkrYhZ4WmzFjvQrp7sDa4ECUxTi9oby8K4FZkd3XCBtEdKwUiQyYJaxiJo5y42gyDWEczrFpozEjeLxMPxjf2WtkfcbpUdfvNnozWF/<0;1>/*),older(10))))#d72le4dr";
        let wallet = Arc::new(Wallet::new(
            CoincubeDescriptor::from_str(DESC).expect("fixture descriptor"),
        ));
        let outpoint = |n: u32| OutPoint::new(Txid::from_str(&format!("{n:064x}")).unwrap(), 0);
        let address = Address::from_str("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4")
            .unwrap()
            .assume_checked();
        let coin = |n: u32, sats: u64, spent: bool| Coin {
            amount: Amount::from_sat(sats),
            outpoint: outpoint(n),
            address: address.clone(),
            block_height: Some(100),
            derivation_index: ChildNumber::from(n),
            spend_info: spent.then(|| LCSpendInfo {
                txid: outpoint(99).txid,
                height: Some(110),
            }),
            is_immature: false,
            is_change: false,
            is_from_self: false,
        };
        let signature_split = outpoint(1);
        let verified = outpoint(2);
        let spent_split = outpoint(3);
        let both = outpoint(4);

        let mut cache = Cache::default();
        cache.daemon_cache.coins = vec![
            coin(1, 10_000, false),
            coin(2, 20_000, false),
            coin(3, 30_000, true),
            coin(4, 40_000, false),
        ];
        cache.split_unconfirmed.insert(signature_split);
        cache.split_unconfirmed.insert(spent_split);
        cache
            .separated
            .insert(verified, Instant::now() + Duration::from_secs(600));
        assert!(cache.record_entanglement(both.txid, Entanglement::Entangled, Instant::now()));
        assert_eq!(
            cache.chain_status(&signature_split),
            ChainStatus::SplitUnconfirmed
        );
        assert_eq!(cache.chain_status(&verified), ChainStatus::OnlyHere);
        assert_eq!(cache.chain_status(&both), ChainStatus::BothChains);

        let panel = SplitHubPanel::new(wallet);
        let facts = panel.facts(&cache);
        assert_eq!(
            facts.unverified, 1,
            "only the unspent signature split lacks positive evidence"
        );
        assert_eq!(facts.both_chains, 1);
        assert_eq!(facts.both_total, Amount::from_sat(40_000));

        cache
            .daemon_cache
            .coins
            .retain(|coin| coin.outpoint == signature_split || coin.outpoint == verified);
        let facts = panel.facts(&cache);
        assert_eq!(facts.unverified, 1);
        assert_eq!(facts.both_chains, 0);
        let coins_items = items(facts);
        assert_eq!(coins_items[0].state, ItemState::ToDo);
        assert!(coins_items[0].detail.starts_with("1 coin not verified yet"));
        assert_eq!(coins_items[0].action, None);

        cache
            .daemon_cache
            .coins
            .retain(|coin| coin.outpoint == verified);
        cache.split_unconfirmed.clear();
        let facts = panel.facts(&cache);
        assert_eq!(facts.unverified, 0);
        let done = items(facts);
        assert_eq!(done[0].state, ItemState::Done);
        assert!(done[0].detail.starts_with("None."));
    }

    /// An imported Vault gets the same checklist on both chains. Its signers
    /// make ordinary signatures only, so it always needs a split key. On
    /// Bitcoin it offers Create copy, which makes its paired imported Cube.
    #[test]
    fn imported_vault_hub_needs_a_split_key_and_offers_its_copy_on_bitcoin() {
        use crate::services::split_test_wallets::{imported_fork_vault, imported_vault, SHAPES};
        for shape in SHAPES {
            let bitcoin = imported_vault(shape);
            let fork = imported_fork_vault(shape);
            assert_eq!(fork.chain, ChainId::BitcoinBlake2b);
            for wallet in [bitcoin, fork] {
                let here = wallet.chain;
                let panel = SplitHubPanel::new(Arc::new(wallet));
                let facts = HubFacts {
                    here,
                    both_chains: 1,
                    both_total: Amount::from_sat(50_000),
                    ..panel.facts(&Cache::default())
                };
                assert!(!facts.signature_available, "{:?} {:?}", shape, here);
                assert!(facts.can_create_copy, "{:?} {:?}", shape, here);
                let items = items(facts);
                assert_eq!(items[0].action, Some(("Split coins", HubAction::Split)));
                let key = items
                    .iter()
                    .find(|item| item.title.starts_with("Split key on"))
                    .unwrap();
                assert_eq!(key.state, ItemState::ToDo);
                assert_eq!(key.action, Some(("Get a split key", HubAction::Split)));
                if here.is_blake2b() {
                    assert_eq!(items[1].title, "Replay-protected signature");
                    assert_eq!(items[1].state, ItemState::Optional);
                    assert!(items
                        .iter()
                        .all(|item| item.action != Some(("Create copy", HubAction::CreateCopy))));
                } else {
                    assert_eq!(items[2].title, "Bitcoin Blake2b copy");
                    assert_eq!(items[2].state, ItemState::Optional);
                    assert_eq!(
                        items[2].action,
                        Some(("Create copy", HubAction::CreateCopy))
                    );
                }
            }
        }
        // With the paired Cube in place the copy line is checked off.
        let items = items(HubFacts {
            copy_exists: true,
            can_create_copy: true,
            ..facts()
        });
        assert_eq!(items[2].state, ItemState::Done);
        assert_eq!(items[2].action, None);
    }

    #[test]
    fn hub_mounts_its_checklist() {
        use crate::app::view::shared::landing::capture;
        let mounted = capture::inspect(
            "vault-split-coins-hub",
            hub_view(ChainId::Bitcoin, items(facts())),
            (1024, 768),
            "dark",
            coincube_ui::theme::Theme::dark(),
        );
        assert!(mounted.contains("Split coins"));
        assert!(mounted.contains("Coins on both chains"));
        assert!(mounted.contains("3 coins · 1,200,000 sats"));
        assert!(mounted.contains("Split key on Bitcoin"));
        assert!(mounted.contains("Create copy"));
    }
}

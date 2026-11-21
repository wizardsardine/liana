use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::{cmp::Ordering, collections::HashSet};

use coincube_core::miniscript::bitcoin::OutPoint;

use iced::Task;

use coincube_ui::widget::Element;
use coincubed::commands::CoinStatus;

use crate::daemon::model::LabelsLoader;
use crate::{
    app::{
        cache::Cache,
        error::Error,
        menu::Menu,
        message::Message,
        state::{vault::label::LabelsEdited, State},
        view::{self, CoinControlMessage},
        wallet::Wallet,
    },
    daemon::{
        model::{Coin, LabelItem, Labelled},
        Daemon,
    },
    services::split_coins,
};

#[derive(Debug, Default)]
pub struct Coins {
    list: Vec<Coin>,
    labels: HashMap<String, String>,
}

impl Labelled for Coins {
    fn labelled(&self) -> Vec<LabelItem> {
        let mut items = Vec::new();
        for coin in &self.list {
            items.push(LabelItem::OutPoint(coin.outpoint));
            items.push(LabelItem::Txid(coin.outpoint.txid));
            items.push(LabelItem::Address(coin.address.clone()));
        }
        items
    }
    fn labels(&mut self) -> &mut HashMap<String, String> {
        &mut self.labels
    }
}

pub struct CoinsPanel {
    coins: Coins,
    selected: Vec<usize>,
    labels_edited: LabelsEdited,
    label_saving: Option<OutPoint>,
    warning: Option<Error>,
    /// timelock value to pass for the heir to consume a coin.
    timelock: Option<u16>,
    /// The Cube's chain, for other-chain deposit status on the fork pair.
    chain: crate::chain::ChainId,
    wallet: Option<Arc<Wallet>>,
    /// Coins ticked in the table, for Send selected.
    chosen: BTreeSet<OutPoint>,
    /// The warning before sending coins that are on both chains; `Some(ack)`
    /// while shown.
    send_warning: Option<bool>,
}

impl CoinsPanel {
    pub fn new(
        coins: &[Coin],
        timelock: impl Into<Option<u16>>,
        chain: crate::chain::ChainId,
    ) -> Self {
        let mut panel = Self {
            labels_edited: LabelsEdited::default(),
            label_saving: None,
            coins: Coins::default(),
            selected: Vec::new(),
            warning: None,
            timelock: timelock.into(),
            chain,
            wallet: None,
            chosen: BTreeSet::new(),
            send_warning: None,
        };
        panel.update_coins(coins);
        panel
    }

    pub fn with_wallet(mut self, wallet: Arc<Wallet>) -> Self {
        self.wallet = Some(wallet);
        self
    }

    /// Whether the Chains column and split surfaces show: a Blake2b Cube,
    /// or a Bitcoin Cube with Bitcoin Blake2b enabled.
    fn blake2b(&self, cache: &Cache) -> bool {
        self.chain.is_blake2b()
            || (matches!(
                self.chain,
                crate::chain::ChainId::Bitcoin | crate::chain::ChainId::Testnet4
            ) && cache.btcb2_server_enabled)
    }

    /// Confirmed coins of this table still on both chains.
    fn both_chain_coins<'a>(&'a self, cache: &'a Cache) -> impl Iterator<Item = &'a Coin> + 'a {
        self.coins.list.iter().filter(move |coin| {
            coin.block_height.is_some() && cache.chain_status(&coin.outpoint).needs_split()
        })
    }

    fn signature_available(&self) -> bool {
        self.wallet
            .as_deref()
            .is_some_and(split_coins::signature_available)
    }

    /// Whether Send selected must warn first: Bitcoin Blake2b is on and a
    /// chosen coin is still on both chains. On Bitcoin a send can copy to
    /// Bitcoin Blake2b; on Bitcoin Blake2b only a Vault that can't make the
    /// replay-protected signature is exposed. The warning offers Split first
    /// and never blocks: the user may send anyway after the tick.
    fn send_needs_warning(&self, cache: &Cache) -> bool {
        self.blake2b(cache)
            && (!self.chain.is_blake2b() || !self.signature_available())
            && self
                .both_chain_coins(cache)
                .any(|coin| self.chosen.contains(&coin.outpoint))
    }

    fn update_split(&mut self, cache: &Cache, message: CoinControlMessage) -> Task<Message> {
        match message {
            CoinControlMessage::EditLabel(outpoint) => {
                return self.edit_coin_label(outpoint);
            }
            CoinControlMessage::Choose(outpoint) => {
                if !self.chosen.remove(&outpoint) {
                    self.chosen.insert(outpoint);
                }
            }
            CoinControlMessage::ClearChosen => self.chosen.clear(),
            CoinControlMessage::SendSelected => {
                return self.send_selected(cache);
            }
            CoinControlMessage::AcknowledgeBothChains(ack) => {
                if let Some(state) = self.send_warning.as_mut() {
                    *state = ack;
                }
            }
            CoinControlMessage::SendAnyway => {
                if self.send_warning == Some(true) {
                    self.send_warning = None;
                    return Task::done(Message::SendSelectedCoins(
                        self.chosen.iter().copied().collect(),
                    ));
                }
            }
            CoinControlMessage::CloseWarning => self.send_warning = None,
            CoinControlMessage::OpenSplit => {
                // The split flow is Tools › Split coins.
                self.send_warning = None;
                return Task::done(Message::View(view::Message::SplitFirst));
            }
        }
        Task::none()
    }

    fn edit_coin_label(&mut self, outpoint: OutPoint) -> Task<Message> {
        if self.label_saving.is_some()
            || !self.coins.list.iter().any(|coin| coin.outpoint == outpoint)
        {
            return Task::none();
        }
        self.labels_edited = LabelsEdited::default();
        let value = self
            .coins
            .labels
            .get(&outpoint.to_string())
            .cloned()
            .unwrap_or_default();
        Task::done(Message::View(view::Message::Label(
            vec![outpoint.to_string()],
            view::LabelMessage::Edited(value),
        )))
        .chain(iced::widget::operation::focus(
            view::vault::coins::COIN_LABEL_INPUT_ID,
        ))
    }

    fn send_selected(&mut self, cache: &Cache) -> Task<Message> {
        if !self.chosen.is_empty() {
            if self.send_needs_warning(cache) {
                self.send_warning = Some(false);
                return Task::none();
            }
            return Task::done(Message::SendSelectedCoins(
                self.chosen.iter().copied().collect(),
            ));
        }
        Task::none()
    }

    fn prepare_coin_label_update(&mut self, message: &Message) -> Option<(Option<String>, bool)> {
        let mut restore_focus = None;
        let saved = match message {
            Message::View(view::Message::Label(items, intent)) => {
                if self.label_saving.is_some() {
                    return None;
                }
                match intent {
                    view::LabelMessage::Confirm => {
                        let item = items.first().filter(|item| {
                            self.labels_edited
                                .cache()
                                .get(*item)
                                .is_some_and(|value| value.valid)
                        })?;
                        self.label_saving = item.parse().ok();
                    }
                    view::LabelMessage::Cancel => restore_focus = items.first().cloned(),
                    _ => {}
                }
                false
            }
            Message::LabelsUpdated(result) => {
                let pending = self.label_saving.take();
                if let (Some(outpoint), Ok(labels)) = (pending, result) {
                    if labels.contains_key(&outpoint.to_string()) {
                        restore_focus = Some(outpoint.to_string());
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            }
            _ => false,
        };
        Some((restore_focus, saved))
    }

    fn update_coin_label(
        &mut self,
        daemon: Arc<dyn Daemon + Sync + Send>,
        message: Message,
    ) -> Task<Message> {
        let Some((restore_focus, saved)) = self.prepare_coin_label_update(&message) else {
            return Task::none();
        };
        match self.labels_edited.update(
            daemon,
            message,
            std::iter::once(&mut self.coins).map(|a| a as &mut dyn LabelsLoader),
        ) {
            Ok(cmd) => {
                let focus = restore_focus
                    .map(|outpoint| {
                        iced::widget::operation::focus(view::vault::coins::coin_action_id(
                            &outpoint, "label",
                        ))
                    })
                    .unwrap_or_else(Task::none);
                let notice = if saved {
                    Task::done(Message::View(view::Message::ShowToast(
                        log::Level::Info,
                        "Coin label saved".to_string(),
                    )))
                } else {
                    Task::none()
                };
                Task::batch([cmd, focus, notice])
            }
            Err(e) => {
                let err_msg = crate::user_error::report(&e);
                self.warning = Some(e);
                Task::done(Message::View(view::Message::ShowError(err_msg)))
            }
        }
    }

    fn update_coins(&mut self, coins: &[Coin]) {
        self.coins.list = coins
            .iter()
            .filter(|coin| coin.spend_info.is_none())
            .cloned()
            .collect();

        self.coins
            .list
            .sort_by(|a, b| match (a.block_height, b.block_height) {
                (Some(a_height), Some(b_height)) => {
                    if a_height == b_height {
                        a.outpoint.vout.cmp(&b.outpoint.vout)
                    } else {
                        a_height.cmp(&b_height)
                    }
                }
                (None, Some(_)) => Ordering::Greater,
                (Some(_), None) => Ordering::Less,
                (None, None) => a.outpoint.vout.cmp(&b.outpoint.vout),
            })
    }
}

impl State for CoinsPanel {
    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, view::Message> {
        let blake2b = self.blake2b(cache);
        let both: Vec<&Coin> = if blake2b {
            self.both_chain_coins(cache).collect()
        } else {
            Vec::new()
        };
        let control = view::vault::coins::CoinControl {
            chosen: self.chosen.clone(),
            both_chains: (!both.is_empty())
                .then(|| (both.len(), both.iter().map(|coin| coin.amount).sum(), true)),
            selection_bar: true,
            label_saving: self.label_saving,
        };
        let content = view::dashboard(
            menu,
            cache,
            view::vault::coins::coins_view(
                cache,
                &self.coins.list,
                self.timelock,
                &self.selected,
                &self.coins.labels,
                self.labels_edited.cache(),
                cache.bitcoin_unit,
                blake2b,
                control,
            ),
        );
        if let Some(ack) = self.send_warning {
            let count = self
                .both_chain_coins(cache)
                .filter(|coin| self.chosen.contains(&coin.outpoint))
                .count();
            return coincube_ui::widget::modal::Modal::new(
                content,
                view::vault::split_coins::send_warning(
                    self.chain,
                    count,
                    ack,
                    self.wallet.as_deref().is_some_and(Wallet::is_imported),
                ),
            )
            .on_blur(Some(view::Message::CoinControl(
                CoinControlMessage::CloseWarning,
            )))
            .into();
        }
        content
    }

    fn update(
        &mut self,
        daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        let daemon = daemon.expect("Daemon required for vault coins panel");
        match message {
            Message::View(view::Message::CoinControl(message)) => {
                return self.update_split(cache, message);
            }
            Message::Coins(res) => match res {
                Err(e) => {
                    let err_msg = crate::user_error::report(&e);
                    self.warning = Some(e);
                    return Task::done(Message::View(view::Message::ShowError(err_msg)));
                }
                Ok(coins) => {
                    self.selected = Vec::new();
                    self.warning = None;
                    self.update_coins(&coins);
                    let held: HashSet<OutPoint> =
                        self.coins.list.iter().map(|coin| coin.outpoint).collect();
                    self.chosen.retain(|outpoint| held.contains(outpoint));
                }
            },
            Message::Labels(res) => match res {
                Err(e) => {
                    let err_msg = crate::user_error::report(&e);
                    self.warning = Some(e);
                    return Task::done(Message::View(view::Message::ShowError(err_msg)));
                }
                Ok(labels) => {
                    self.coins.labels = labels;
                }
            },
            Message::View(view::Message::Label(_, _)) | Message::LabelsUpdated(_) => {
                return self.update_coin_label(daemon, message);
            }
            Message::View(view::Message::Select(i)) => {
                if let Some(position) = self.selected.iter().position(|j| *j == i) {
                    self.selected.remove(position);
                } else {
                    self.selected.push(i);
                }
            }
            _ => {}
        }
        Task::none()
    }

    fn reload(
        &mut self,
        daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        _wallet: Option<Arc<Wallet>>,
    ) -> Task<Message> {
        let daemon = daemon.expect("Vault panels require daemon");
        let daemon1 = daemon.clone();
        let daemon2 = daemon.clone();
        Task::batch(vec![
            Task::perform(
                async move {
                    daemon1
                        .list_coins(&[CoinStatus::Unconfirmed, CoinStatus::Confirmed], &[])
                        .await
                        .map(|res| res.coins)
                        .map_err(|e| e.into())
                },
                Message::Coins,
            ),
            Task::perform(
                async move {
                    let coins = daemon2
                        .list_coins(&[CoinStatus::Unconfirmed, CoinStatus::Confirmed], &[])
                        .await
                        .map(|res| res.coins)
                        .map_err(Error::from)?;
                    let mut targets = HashSet::<LabelItem>::new();
                    for coin in coins {
                        targets.insert(LabelItem::OutPoint(coin.outpoint));
                        targets.insert(LabelItem::Txid(coin.outpoint.txid));
                        targets.insert(LabelItem::Address(coin.address));
                    }
                    daemon2.get_labels(&targets).await.map_err(|e| e.into())
                },
                Message::Labels,
            ),
        ])
    }
}

impl From<CoinsPanel> for Box<dyn State> {
    fn from(s: CoinsPanel) -> Box<dyn State> {
        Box::new(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::model::Coin;
    use coincube_core::miniscript::bitcoin;
    use std::str::FromStr;

    async fn outputs(task: Task<Message>) -> Vec<Message> {
        use iced::futures::StreamExt;

        let mut messages = Vec::new();
        if let Some(mut stream) = iced_runtime::task::into_stream(task) {
            while let Some(action) = stream.next().await {
                if let iced_runtime::Action::Output(message) = action {
                    messages.push(message);
                }
            }
        }
        messages
    }

    #[tokio::test]
    async fn coin_label_saves_retry_preserve_scope_and_reject_invalid_or_duplicate_writes() {
        use crate::daemon::{client::Coincubed, DaemonError};
        use crate::utils::mock::Daemon as MockDaemon;
        use serde_json::json;
        let coin = Coin {
            outpoint: OutPoint::from_str(&format!("{:064x}:0", 1)).unwrap(),
            amount: bitcoin::Amount::from_sat(100_000),
            block_height: Some(100),
            spend_info: None,
            is_immature: false,
            address: bitcoin::Address::from_str("bc1qvrl2849aggm6qry9ea7xqp2kk39j8vaa8r3cwg")
                .unwrap()
                .assume_checked(),
            derivation_index: 0.into(),
            is_change: false,
            is_from_self: false,
        };
        let outpoint = coin.outpoint.to_string();
        let expected = json!({"method":"updatelabels","params":[{(outpoint.clone()):"Reserve"}]});
        let daemon: Arc<dyn Daemon + Sync + Send> = Arc::new(Coincubed::new(
            MockDaemon::new(vec![
                (Some(expected.clone()), Err(DaemonError::NoAnswer)),
                (Some(expected), Ok(json!({}))),
                (
                    Some(json!({"method":"updatelabels","params":[{(outpoint.clone()):null}]})),
                    Ok(json!({})),
                ),
            ])
            .run(),
        ));
        let cache = Cache::default();
        let mut panel = CoinsPanel::new(
            std::slice::from_ref(&coin),
            Some(1008),
            crate::chain::ChainId::Bitcoin,
        );
        panel
            .coins
            .labels
            .insert(coin.outpoint.txid.to_string(), "Payroll".into());
        panel
            .coins
            .labels
            .insert(coin.address.to_string(), "Deposit address".into());
        let open = panel.update(
            Some(daemon.clone()),
            &cache,
            Message::View(view::Message::CoinControl(CoinControlMessage::EditLabel(
                coin.outpoint,
            ))),
        );
        for message in outputs(open).await {
            let _ = panel.update(Some(daemon.clone()), &cache, message);
        }
        assert!(panel.selected.is_empty(), "editing must not expand details");
        assert_eq!(panel.labels_edited.cache()[&outpoint].value, "");
        let edit = |value| {
            Message::View(view::Message::Label(
                vec![outpoint.clone()],
                view::LabelMessage::Edited(value),
            ))
        };
        let confirm = || {
            Message::View(view::Message::Label(
                vec![outpoint.clone()],
                view::LabelMessage::Confirm,
            ))
        };
        let _ = panel.update(Some(daemon.clone()), &cache, edit("x".repeat(101)));
        assert!(!panel.labels_edited.cache()[&outpoint].valid);
        let task = panel.update(Some(daemon.clone()), &cache, confirm());
        assert!(
            iced_runtime::task::into_stream(task).is_none(),
            "invalid draft must not call daemon"
        );
        for succeeds in [false, true] {
            let _ = panel.update(Some(daemon.clone()), &cache, edit("Reserve".into()));
            let save = panel.update(Some(daemon.clone()), &cache, confirm());
            assert_eq!(panel.label_saving, Some(coin.outpoint));
            assert!(
                iced_runtime::task::into_stream(panel.update(
                    Some(daemon.clone()),
                    &cache,
                    confirm()
                ))
                .is_none(),
                "pending save must not submit twice"
            );
            for message in outputs(save).await {
                let notice = panel.update(Some(daemon.clone()), &cache, message);
                let messages = outputs(notice).await;
                assert_eq!(messages.iter().any(|message| matches!(message, Message::View(view::Message::ShowToast(_, text)) if text == "Coin label saved")), succeeds);
            }
            assert_eq!(panel.label_saving, None);
            if succeeds {
                assert!(!panel.labels_edited.cache().contains_key(&outpoint));
                assert_eq!(panel.coins.labels[&outpoint], "Reserve");
            } else {
                assert_eq!(panel.labels_edited.cache()[&outpoint].value, "Reserve");
                assert!(!panel.coins.labels.contains_key(&outpoint));
            }
            assert_eq!(
                panel.coins.labels[&coin.outpoint.txid.to_string()],
                "Payroll"
            );
            assert_eq!(
                panel.coins.labels[&coin.address.to_string()],
                "Deposit address"
            );
        }
        // Saving an empty label removes only the coin's override.
        let _ = panel.update(Some(daemon.clone()), &cache, edit("".into()));
        let save = panel.update(Some(daemon.clone()), &cache, confirm());
        for message in outputs(save).await {
            let _ = panel.update(Some(daemon.clone()), &cache, message);
        }
        assert!(!panel.coins.labels.contains_key(&outpoint));
        assert_eq!(
            panel.coins.labels[&coin.outpoint.txid.to_string()],
            "Payroll"
        );
        let _ = panel.update(Some(daemon.clone()), &cache, edit("Unsaved draft".into()));
        let _ = panel.update(
            Some(daemon),
            &cache,
            Message::View(view::Message::Label(
                vec![outpoint.clone()],
                view::LabelMessage::Cancel,
            )),
        );
        assert!(panel.labels_edited.cache().is_empty());
        assert!(!panel.coins.labels.contains_key(&outpoint));
    }

    #[test]
    fn test_coins_panel_update_coins() {
        let mut panel = CoinsPanel::new(&[], 0, crate::chain::ChainId::Bitcoin);
        let txid = bitcoin::Txid::from_str(
            "f7bd1b2a995b689d326e51eb742eb1088c4a8f110d9cb56128fd553acc9f88e5",
        )
        .unwrap();
        let dummy_address =
            bitcoin::Address::from_str("bc1qvrl2849aggm6qry9ea7xqp2kk39j8vaa8r3cwg")
                .unwrap()
                .assume_checked();

        panel.update_coins(&[
            Coin {
                outpoint: bitcoin::OutPoint { txid, vout: 2 },
                amount: bitcoin::Amount::from_sat(1),
                block_height: Some(3),
                spend_info: None,
                is_immature: false,
                address: dummy_address.clone(),
                derivation_index: 0.into(),
                is_change: false,
                is_from_self: false,
            },
            Coin {
                outpoint: bitcoin::OutPoint { txid, vout: 3 },
                amount: bitcoin::Amount::from_sat(1),
                block_height: None,
                spend_info: None,
                is_immature: false,
                address: dummy_address.clone(),
                derivation_index: 1.into(),
                is_change: false,
                is_from_self: false,
            },
            Coin {
                outpoint: bitcoin::OutPoint { txid, vout: 0 },
                amount: bitcoin::Amount::from_sat(1),
                block_height: Some(2),
                spend_info: None,
                is_immature: false,
                address: dummy_address.clone(),
                derivation_index: 2.into(),
                is_change: false,
                is_from_self: false,
            },
            Coin {
                outpoint: bitcoin::OutPoint { txid, vout: 1 },
                amount: bitcoin::Amount::from_sat(1),
                block_height: Some(3),
                spend_info: None,
                is_immature: false,
                address: dummy_address,
                derivation_index: 3.into(),
                is_change: false,
                is_from_self: false,
            },
        ]);

        assert_eq!(
            panel
                .coins
                .list
                .iter()
                .map(|c| c.outpoint)
                .collect::<Vec<bitcoin::OutPoint>>(),
            vec![
                bitcoin::OutPoint { txid, vout: 0 },
                bitcoin::OutPoint { txid, vout: 1 },
                bitcoin::OutPoint { txid, vout: 2 },
                bitcoin::OutPoint { txid, vout: 3 },
            ]
        )
    }

    mod split_surfaces {
        use super::super::*;
        use crate::{
            app::{cache::Cache, view::CoinControlMessage},
            services::{chain_status::ProofEntry, entangled::Entanglement},
        };
        use coincube_core::{
            claim_ancestry::{retained::RetainedPath, search::OwnedLink},
            miniscript::bitcoin::{
                self, absolute, consensus::serialize, script::Builder, transaction, Amount,
                ScriptBuf, Sequence, Transaction, TxIn, TxOut, Witness,
            },
        };
        use std::str::FromStr;

        fn coin(outpoint: OutPoint, sats: u64) -> Coin {
            Coin {
                outpoint,
                amount: Amount::from_sat(sats),
                block_height: Some(100),
                spend_info: None,
                is_immature: false,
                address: bitcoin::Address::from_str("bc1qvrl2849aggm6qry9ea7xqp2kk39j8vaa8r3cwg")
                    .unwrap()
                    .assume_checked(),
                derivation_index: 0.into(),
                is_change: false,
                is_from_self: false,
            }
        }

        /// Two both-chain coins, one verified split key and one coin the
        /// twin chain lacks without proof.
        fn fixture() -> (CoinsPanel, Cache, [OutPoint; 4]) {
            let coinbase = Transaction {
                version: transaction::Version::TWO,
                lock_time: absolute::LockTime::ZERO,
                input: vec![TxIn {
                    previous_output: OutPoint::null(),
                    script_sig: Builder::new().push_int(962_000).push_int(1).into_script(),
                    sequence: Sequence::MAX,
                    witness: Witness::new(),
                }],
                output: vec![TxOut {
                    value: Amount::from_sat(30_000),
                    script_pubkey: ScriptBuf::new(),
                }],
            };
            let shared = |n: u8| {
                OutPoint::from_str(&format!("{}:0", format!("{:02x}", n).repeat(32))).unwrap()
            };
            let (a, b, unseen) = (shared(1), shared(2), shared(3));
            let key = OutPoint::new(coinbase.compute_txid(), 0);
            let coins = vec![
                coin(a, 100_000),
                coin(b, 100_000),
                coin(key, 30_000),
                coin(unseen, 5_000),
            ];
            let mut cache = Cache::default();
            cache.daemon_cache.coins = coins.clone();
            let now = std::time::Instant::now();
            cache.record_entanglement(a.txid, Entanglement::Entangled, now);
            cache.record_entanglement(b.txid, Entanglement::Entangled, now);
            cache.record_entanglement(key.txid, Entanglement::NotEntangled, now);
            cache.record_entanglement(unseen.txid, Entanglement::NotEntangled, now);
            cache.chain_proofs.insert(
                key,
                ProofEntry::Verified(
                    RetainedPath::new(
                        key,
                        vec![OwnedLink {
                            transaction: serialize(&coinbase),
                            parent_input: None,
                        }],
                    )
                    .unwrap(),
                ),
            );
            let panel = CoinsPanel::new(&coins, 0, crate::chain::ChainId::Bitcoin);
            (panel, cache, [a, b, key, unseen])
        }

        /// Coin control's Split coins goes to Tools › Split coins and keeps
        /// the table's selection.
        #[test]
        fn split_coins_goes_to_the_split_coins_tab() {
            let (mut panel, cache, [_, _, _, unseen]) = fixture();
            let _ = panel.update_split(&cache, CoinControlMessage::Choose(unseen));
            let task = panel.update_split(&cache, CoinControlMessage::OpenSplit);
            assert!(task.units() > 0, "asks the App for the tab");
            assert_eq!(
                panel.chosen,
                vec![unseen].into_iter().collect::<BTreeSet<_>>()
            );
        }

        #[test]
        fn sending_both_chain_coins_warns_and_needs_an_acknowledgement() {
            let (mut panel, mut cache, [a, _, _, unseen]) = fixture();
            cache.btcb2_server_enabled = true;
            let _ = panel.update_split(&cache, CoinControlMessage::Choose(unseen));
            let _ = panel.update_split(&cache, CoinControlMessage::SendSelected);
            assert_eq!(panel.send_warning, None, "a one-chain coin sends at once");
            let _ = panel.update_split(&cache, CoinControlMessage::Choose(a));
            let _ = panel.update_split(&cache, CoinControlMessage::SendSelected);
            assert_eq!(panel.send_warning, Some(false));
            let _ = panel.update_split(&cache, CoinControlMessage::SendAnyway);
            assert_eq!(panel.send_warning, Some(false), "not without the tick");
            let _ = panel.update_split(&cache, CoinControlMessage::AcknowledgeBothChains(true));
            let _ = panel.update_split(&cache, CoinControlMessage::SendAnyway);
            assert_eq!(panel.send_warning, None);
            // Split first closes the warning and goes to Split coins.
            let _ = panel.update_split(&cache, CoinControlMessage::SendSelected);
            let task = panel.update_split(&cache, CoinControlMessage::OpenSplit);
            assert_eq!(panel.send_warning, None);
            assert!(task.units() > 0);
        }

        #[test]
        fn no_warning_with_bitcoin_blake2b_off() {
            let (mut panel, cache, [a, ..]) = fixture();
            let _ = panel.update_split(&cache, CoinControlMessage::Choose(a));
            let _ = panel.update_split(&cache, CoinControlMessage::SendSelected);
            assert_eq!(panel.send_warning, None);
        }

        #[test]
        fn a_blake2b_vault_without_the_signature_sends_anyway_after_the_tick() {
            let (_, mut cache, [a, ..]) = fixture();
            let coins = cache.daemon_cache.coins.clone();
            let mut panel = CoinsPanel::new(&coins, 0, crate::chain::ChainId::BitcoinBlake2b);
            cache.fiat_chain = crate::chain::ChainId::BitcoinBlake2b;
            let _ = panel.update_split(&cache, CoinControlMessage::Choose(a));
            let _ = panel.update_split(&cache, CoinControlMessage::SendSelected);
            assert_eq!(panel.send_warning, Some(false));
            let _ = panel.update_split(&cache, CoinControlMessage::SendAnyway);
            assert_eq!(panel.send_warning, Some(false), "not without the tick");
            let _ = panel.update_split(&cache, CoinControlMessage::AcknowledgeBothChains(true));
            let task = panel.update_split(&cache, CoinControlMessage::SendAnyway);
            assert_eq!(panel.send_warning, None, "replay is the user's choice");
            assert!(task.units() > 0, "the send goes ahead");
        }
    }
}

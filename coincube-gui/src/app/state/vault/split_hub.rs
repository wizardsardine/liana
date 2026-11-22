//! Tools › Split coins: the whole split flow, one step at a time (Tenshu
//! canvas, `splitflow` page). The step is derived from the Vault each time
//! the tab is drawn: get a split coin, review and sign, confirmations, then
//! done or the coins still left. Only the user's choices (coins left out, the
//! method), the address to send a split coin to, the fee rate and the build in
//! flight are kept here. Waiting for a split coin can take an hour, so the
//! flow lives in the tab rather than a dialog: you can leave and come back.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use coincube_core::miniscript::bitcoin::{OutPoint, Txid};
use coincube_ui::widget::Element;
use coincubed::commands::CoinStatus;
use iced::{widget::qr_code, Task};

use crate::{
    app::{
        cache::Cache,
        error::Error,
        menu::Menu,
        message::Message,
        state::State,
        view::{
            self,
            vault::split_coins::{CopyCard, KeyStatus, ReviewCoin, SplitFlow, Step, MIN_KEY_SATS},
            Message as ViewMessage, SplitMessage,
        },
        wallet::Wallet,
    },
    daemon::{
        model::{Coin, LabelItem},
        Daemon,
    },
    services::{
        chain_status::{ChainStatus, SPLIT_CONFIRMATIONS},
        split_coins::{self, Method},
    },
};

/// How far back, in blocks, a confirmed coin still being checked counts as a
/// split coin that just arrived: about a day.
const RECENT_BLOCKS: i32 = 144;

/// Unavailable fee rates retried on their own before the review offers
/// **Try again**: one failed Connect read shouldn't leave the user stuck.
const FEE_AUTO_RETRIES: u8 = 3;

/// The wait before each automatic fee-rate retry.
const FEE_RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(5);

/// The facts the step is derived from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlowFacts {
    /// Confirmed unspent coins still on both chains.
    pub both: Vec<OutPoint>,
    /// Unspent coins with no answer yet: not checked, still checking, a check
    /// that failed, or a signature split nothing has confirmed. Only positive
    /// evidence makes the flow done.
    pub unverified: usize,
    /// A split coin ready to spend.
    pub key: bool,
    pub key_status: KeyStatus,
    pub signature_available: bool,
    /// The fewest confirmations among this Vault's sent splits still short of
    /// [`SPLIT_CONFIRMATIONS`].
    pub sending: Option<u32>,
    /// A split built and stored that nothing has sent yet.
    pub unsigned: Option<Txid>,
    /// The both-chain coins unsent splits already spend. They wait for that
    /// split (or its record to expire); the other coins can still be split.
    pub reserved: Vec<OutPoint>,
    /// Coins this Vault's confirmed splits separated.
    pub split_before: usize,
}

/// The step the flow is on. `continuing` is the user's Split the rest.
pub fn step(facts: &FlowFacts, method: Method, continuing: bool) -> Step {
    if let Some(confirmations) = facts.sending {
        return Step::Sent { confirmations };
    }
    let available = facts
        .both
        .iter()
        .filter(|outpoint| !facts.reserved.contains(outpoint))
        .count();
    if let Some(txid) = facts.unsigned.filter(|_| available == 0) {
        return Step::Unsigned { txid };
    }
    if facts.both.is_empty() {
        return if facts.unverified > 0 {
            Step::Verifying {
                count: facts.unverified,
            }
        } else {
            Step::Done
        };
    }
    if facts.split_before > 0 && !continuing {
        return Step::Rest {
            split: facts.split_before,
        };
    }
    if method == Method::SplitKey && !facts.key {
        return Step::Key(facts.key_status);
    }
    Step::Review
}

/// Where a split coin stands while the flow waits for one. `address` is the
/// address the flow shows; a coin received there is the one being waited
/// for. Without it (after a restart), an unconfirmed incoming coin, or a
/// recent confirmed one still being checked, stands in.
pub fn key_status(cache: &Cache, address: Option<&str>) -> KeyStatus {
    let split_txids: HashSet<Txid> = cache
        .split_records
        .iter()
        .map(|record| record.txid)
        .collect();
    let candidates: Vec<&Coin> = cache
        .coins()
        .iter()
        .filter(|coin| coin.spend_info.is_none() && !coin.is_change && !coin.is_from_self)
        .filter(|coin| !split_txids.contains(&coin.outpoint.txid))
        .collect();
    let tip = cache.blockheight();
    let classify = |coin: &Coin| -> KeyStatus {
        let amount = coin.amount;
        let Some(height) = coin.block_height else {
            return KeyStatus::Confirming { amount };
        };
        let confirmations = (tip - height + 1).max(1) as u32;
        match cache.chain_status(&coin.outpoint) {
            ChainStatus::BothChains => KeyStatus::BothChains { amount },
            ChainStatus::CouldntVerify => KeyStatus::CouldntVerify { amount },
            _ => KeyStatus::Checking {
                amount,
                confirmations,
            },
        }
    };
    let at_address = |coin: &&&Coin| address.is_some_and(|a| coin.address.to_string() == a);
    let incoming = |coin: &&&Coin| {
        coin.amount.to_sat() >= MIN_KEY_SATS
            && (coin.block_height.is_none()
                || (coin
                    .block_height
                    .is_some_and(|height| tip - height < RECENT_BLOCKS)
                    && matches!(
                        cache.chain_status(&coin.outpoint),
                        ChainStatus::NotChecked | ChainStatus::NotSeen
                    )))
    };
    let pending = |status: &KeyStatus| {
        matches!(
            status,
            KeyStatus::Confirming { .. } | KeyStatus::Checking { .. }
        )
    };
    // A coin still on its way (at the address, or any new incoming one)
    // wins over one that failed: after "send one from another source" the
    // replacement must show, not the coin that couldn't be a split coin.
    let statuses: Vec<KeyStatus> = candidates
        .iter()
        .filter(|coin| at_address(coin) || incoming(coin))
        .map(|coin| classify(coin))
        .collect();
    if let Some(status) = statuses.iter().find(|status| pending(status)) {
        return *status;
    }
    candidates
        .iter()
        .filter(at_address)
        .map(|coin| classify(coin))
        .next()
        .unwrap_or(KeyStatus::Waiting)
}

/// Tools › Split coins.
pub struct SplitHubPanel {
    wallet: Arc<Wallet>,
    labels: HashMap<String, String>,
    /// Both-chain coins the user left out.
    excluded: BTreeSet<OutPoint>,
    /// The method the user chose; `None` takes the default.
    method: Option<Method>,
    /// The user chose Split the rest after an earlier split.
    continuing: bool,
    feerate: Option<u64>,
    fee_failed: bool,
    /// Automatic retries spent since the last fetch the user or a visit
    /// started.
    fee_retries: u8,
    address: Option<String>,
    qr: Option<qr_code::Data>,
    address_error: Option<String>,
    working: bool,
    error: Option<String>,
}

impl SplitHubPanel {
    pub fn new(wallet: Arc<Wallet>) -> Self {
        Self {
            wallet,
            labels: HashMap::new(),
            excluded: BTreeSet::new(),
            method: None,
            continuing: false,
            feerate: None,
            fee_failed: false,
            fee_retries: 0,
            address: None,
            qr: None,
            address_error: None,
            working: false,
            error: None,
        }
    }

    fn signature_available(&self) -> bool {
        split_coins::signature_available(&self.wallet)
    }

    fn method(&self) -> Method {
        match self.method {
            Some(Method::Signature) | None if self.signature_available() => Method::Signature,
            _ => Method::SplitKey,
        }
    }

    pub fn facts(&self, cache: &Cache) -> FlowFacts {
        let both = cache
            .coins()
            .iter()
            .filter(|coin| coin.spend_info.is_none() && coin.block_height.is_some())
            .filter(|coin| cache.chain_status(&coin.outpoint).needs_split())
            .map(|coin| coin.outpoint)
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
        let sent: HashSet<Txid> = cache
            .coins()
            .iter()
            .map(|coin| coin.outpoint.txid)
            .collect();
        let unsent: Vec<_> = cache
            .split_records
            .iter()
            .filter(|record| record.confirmations == 0 && !sent.contains(&record.txid))
            .collect();
        let unsigned = unsent
            .iter()
            .max_by_key(|record| record.created_at)
            .map(|record| record.txid);
        let reserved = unsent
            .iter()
            .flat_map(|record| record.inputs.iter().copied())
            .collect();
        let split_before = cache
            .split_records
            .iter()
            .filter(|record| record.confirmations >= SPLIT_CONFIRMATIONS)
            .map(|record| record.inputs.len())
            .sum();
        FlowFacts {
            both,
            unverified,
            key: !cache.split_keys().is_empty(),
            key_status: key_status(cache, self.address.as_deref()),
            signature_available: self.signature_available(),
            sending: cache.splitting.values().copied().min(),
            unsigned,
            reserved,
            split_before,
        }
    }

    /// The flow as drawn now.
    pub fn flow(&self, cache: &Cache) -> SplitFlow {
        let facts = self.facts(cache);
        let method = self.method();
        let label = |coin: &Coin| {
            self.labels
                .get(&coin.outpoint.to_string())
                .or_else(|| self.labels.get(&coin.outpoint.txid.to_string()))
                .filter(|label| !label.is_empty())
                .cloned()
                .unwrap_or_else(|| coincube_ui::locale::copy("No label").to_string())
        };
        let coins: Vec<ReviewCoin> = cache
            .coins()
            .iter()
            .filter(|coin| {
                facts.both.contains(&coin.outpoint) && !facts.reserved.contains(&coin.outpoint)
            })
            .map(|coin| ReviewCoin {
                outpoint: coin.outpoint,
                amount: coin.amount,
                label: label(coin),
                included: !self.excluded.contains(&coin.outpoint),
            })
            .collect();
        let included = coins.iter().filter(|coin| coin.included).count();
        // The estimator counts one input beyond the shared ones: the split
        // coin, or for a signature split an overestimate by one input. A
        // Vault and an imported wallet are sized from their own policy.
        let vbytes =
            split_coins::estimate_split_vbytes(&self.wallet.main_descriptor, included.max(1));
        let copy = if self.wallet.chain.is_blake2b() {
            CopyCard::ThisIsTheCopy
        } else if cache.btcb2_already_claimed {
            CopyCard::Exists
        } else if self.wallet.supports_inheritance() || self.wallet.is_imported() {
            CopyCard::Create
        } else {
            CopyCard::None
        };
        let step = step(&facts, method, self.continuing);
        let waiting_to_sign = facts
            .unsigned
            .filter(|_| !matches!(step, Step::Unsigned { .. } | Step::Sent { .. }));
        SplitFlow {
            here: self.wallet.chain,
            step,
            coins,
            key: cache.split_keys().first().map(|(coin, _)| coin.amount),
            method,
            signature_available: facts.signature_available,
            address: self.address.clone(),
            address_error: self.address_error.clone(),
            feerate: self.feerate,
            fee_failed: self.fee_failed,
            vbytes,
            working: self.working,
            error: self.error.clone(),
            copy,
            waiting_to_sign,
            mask_amounts: cache.balance_amounts_masked(),
            unit: cache.bitcoin_unit,
        }
    }

    fn sign(&mut self, cache: &Cache) -> Task<Message> {
        let flow = self.flow(cache);
        if flow.step != Step::Review {
            return Task::none();
        }
        let Some(feerate_vb) = flow.feerate.filter(|_| flow.blocked().is_none()) else {
            return Task::none();
        };
        let coins = flow
            .coins
            .iter()
            .filter(|coin| coin.included)
            .map(|coin| coin.outpoint)
            .collect();
        let key = (flow.method == Method::SplitKey)
            .then(|| cache.split_keys().first().map(|(coin, _)| coin.outpoint))
            .flatten();
        self.working = true;
        self.error = None;
        Task::done(Message::SplitRequest {
            method: flow.method,
            coins,
            key,
            feerate_vb,
        })
    }

    fn set_address(&mut self, result: Result<String, String>) {
        match result {
            Ok(address) => {
                self.qr = qr_code::Data::new(format!("bitcoin:{}", address)).ok();
                self.address = Some(address);
                self.address_error = None;
            }
            Err(e) => self.address_error = Some(e),
        }
    }

    fn set_result(&mut self, result: Result<Box<split_coins::SplitRecord>, String>) {
        self.working = false;
        match result {
            Ok(_) => {
                self.excluded.clear();
                self.continuing = false;
                self.error = None;
            }
            Err(e) => self.error = Some(e),
        }
    }

    /// A build in flight, for the App's routing test.
    #[cfg(test)]
    pub(crate) fn start_building_split_for_test(&mut self) {
        self.working = true;
    }

    /// `(working, error)`, for the App's routing test.
    #[cfg(test)]
    pub(crate) fn split_progress_for_test(&self) -> (bool, Option<String>) {
        (self.working, self.error.clone())
    }
}

impl State for SplitHubPanel {
    fn view<'a>(&'a self, menu: &'a Menu, cache: &'a Cache) -> Element<'a, ViewMessage> {
        view::dashboard(
            menu,
            cache,
            view::vault::split_coins::split_flow_view(&self.flow(cache), self.qr.as_ref()),
        )
    }

    fn update(
        &mut self,
        _daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        cache: &Cache,
        message: Message,
    ) -> Task<Message> {
        match message {
            Message::View(ViewMessage::Split(message)) => match message {
                SplitMessage::ToggleCoin(outpoint) => {
                    if !self.working && !self.excluded.remove(&outpoint) {
                        self.excluded.insert(outpoint);
                    }
                }
                SplitMessage::Method(method) => {
                    if !self.working && (method == Method::SplitKey || self.signature_available()) {
                        self.method = Some(method);
                    }
                }
                SplitMessage::Sign => return self.sign(cache),
                SplitMessage::SplitRest => {
                    self.continuing = true;
                    self.excluded.clear();
                }
                SplitMessage::RetryFee => {
                    if !self.working && self.fee_failed {
                        self.fee_retries = 0;
                        self.fee_failed = false;
                        return Task::done(fetch_fee());
                    }
                }
            },
            Message::SplitFeeRate(rate) => return self.set_feerate(rate),
            Message::SplitAddress(result) => self.set_address(result),
            Message::SplitPrepared(result) => self.set_result(result),
            Message::Labels(Ok(labels)) => self.labels = labels,
            _ => {}
        }
        Task::none()
    }

    fn reload(
        &mut self,
        daemon: Option<Arc<dyn Daemon + Sync + Send>>,
        _wallet: Option<Arc<Wallet>>,
    ) -> Task<Message> {
        // The fee rate on every visit; one address to send a split coin to,
        // kept for the panel's life so the QR code doesn't change under the
        // user. A Vault that splits with the signature never needs one, but
        // the user can switch to a split coin on the review.
        self.fee_retries = 0;
        let fetch = Task::done(Message::SplitFetch {
            fee: true,
            address: self.address.is_none(),
        });
        let Some(daemon) = daemon else {
            return fetch;
        };
        let labels = Task::perform(
            async move {
                let coins = daemon
                    .list_coins(&[CoinStatus::Confirmed], &[])
                    .await
                    .map(|res| res.coins)
                    .map_err(Error::from)?;
                let mut targets = HashSet::<LabelItem>::new();
                for coin in coins {
                    targets.insert(LabelItem::OutPoint(coin.outpoint));
                    targets.insert(LabelItem::Txid(coin.outpoint.txid));
                }
                daemon.get_labels(&targets).await.map_err(|e| e.into())
            },
            Message::Labels,
        );
        Task::batch([fetch, labels])
    }
}

impl SplitHubPanel {
    /// A fee rate, or `None` for unavailable: retried on its own a few times,
    /// still "Getting the fee rate…" meanwhile, before the review says it's
    /// unavailable and offers **Try again**. Never a guess.
    fn set_feerate(&mut self, rate: Option<u64>) -> Task<Message> {
        self.feerate = rate;
        if rate.is_some() {
            self.fee_failed = false;
            self.fee_retries = 0;
            return Task::none();
        }
        if self.fee_retries < FEE_AUTO_RETRIES {
            self.fee_retries += 1;
            self.fee_failed = false;
            return Task::perform(async { tokio::time::sleep(FEE_RETRY_DELAY).await }, |()| {
                fetch_fee()
            });
        }
        self.fee_failed = true;
        Task::none()
    }
}

/// Fetch the fee rate only; the split address is kept for the panel's life.
fn fetch_fee() -> Message {
    Message::SplitFetch {
        fee: true,
        address: false,
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
    use crate::{
        chain::ChainId,
        services::{chain_status::ProofEntry, entangled::Entanglement},
    };
    use coincube_core::{
        claim_ancestry::{retained::RetainedPath, search::OwnedLink},
        descriptors::CoincubeDescriptor,
        miniscript::bitcoin::{
            self, absolute, consensus::serialize, script::Builder, transaction, Amount, ScriptBuf,
            Sequence, Transaction, TxIn, TxOut, Witness,
        },
    };
    use coincubed::commands::LCSpendInfo;
    use std::str::FromStr;

    const DESC: &str = "wsh(or_d(pk([f5acc2fd]tpubD6NzVbkrYhZ4YgUx2ZLNt2rLYAMTdYysCRzKoLu2BeSHKvzqPaBDvf17GeBPnExUVPkuBpx4kniP964e2MxyzzazcXLptxLXModSVCVEV1T/<0;1>/*),and_v(v:pkh([8a64f2a9]tpubD6NzVbkrYhZ4WmzFjvQrp7sDa4ECUxTi9oby8K4FZkd3XCBtEdKwUiQyYJaxiJo5y42gyDWEczrFpozEjeLxMPxjf2WtkfcbpUdfvNnozWF/<0;1>/*),older(10))))#d72le4dr";

    fn wallet() -> Arc<Wallet> {
        Arc::new(Wallet::new(
            CoincubeDescriptor::from_str(DESC).expect("fixture descriptor"),
        ))
    }

    const ADDRESS: &str = "bc1qvrl2849aggm6qry9ea7xqp2kk39j8vaa8r3cwg";

    fn coin(outpoint: OutPoint, sats: u64, height: Option<i32>) -> Coin {
        Coin {
            outpoint,
            amount: Amount::from_sat(sats),
            block_height: height,
            spend_info: None,
            is_immature: false,
            address: bitcoin::Address::from_str(ADDRESS)
                .unwrap()
                .assume_checked(),
            derivation_index: 0.into(),
            is_change: false,
            is_from_self: false,
        }
    }

    fn shared(n: u8) -> OutPoint {
        OutPoint::from_str(&format!("{}:0", format!("{:02x}", n).repeat(32))).unwrap()
    }

    fn key_proof(coinbase: &Transaction, key: OutPoint) -> ProofEntry {
        ProofEntry::Verified(
            RetainedPath::new(
                key,
                vec![OwnedLink {
                    transaction: serialize(coinbase),
                    parent_input: None,
                }],
            )
            .unwrap(),
        )
    }

    fn coinbase() -> Transaction {
        Transaction {
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
        }
    }

    /// Two both-chain coins and one verified split coin.
    fn fixture() -> (SplitHubPanel, Cache, [OutPoint; 3]) {
        let coinbase = coinbase();
        let (a, b) = (shared(1), shared(2));
        let key = OutPoint::new(coinbase.compute_txid(), 0);
        let mut cache = Cache::default();
        cache.daemon_cache.blockheight = 110;
        cache.daemon_cache.coins = vec![
            coin(a, 100_000, Some(100)),
            coin(b, 100_000, Some(100)),
            coin(key, 30_000, Some(100)),
        ];
        let now = std::time::Instant::now();
        cache.record_entanglement(a.txid, Entanglement::Entangled, now);
        cache.record_entanglement(b.txid, Entanglement::Entangled, now);
        cache.record_entanglement(key.txid, Entanglement::NotEntangled, now);
        cache.chain_proofs.insert(key, key_proof(&coinbase, key));
        (SplitHubPanel::new(wallet()), cache, [a, b, key])
    }

    fn facts() -> FlowFacts {
        FlowFacts {
            both: vec![shared(1)],
            unverified: 0,
            key: true,
            key_status: KeyStatus::Waiting,
            signature_available: false,
            sending: None,
            unsigned: None,
            reserved: Vec::new(),
            split_before: 0,
        }
    }

    #[test]
    fn the_step_follows_the_vault() {
        assert_eq!(step(&facts(), Method::SplitKey, false), Step::Review);
        let no_key = FlowFacts {
            key: false,
            ..facts()
        };
        assert_eq!(
            step(&no_key, Method::SplitKey, false),
            Step::Key(KeyStatus::Waiting)
        );
        assert_eq!(
            step(&no_key, Method::Signature, false),
            Step::Review,
            "a signature split needs no split coin"
        );
        let txid = shared(9).txid;
        assert_eq!(
            step(
                &FlowFacts {
                    unsigned: Some(txid),
                    reserved: vec![shared(1)],
                    ..facts()
                },
                Method::SplitKey,
                false
            ),
            Step::Unsigned { txid },
            "nothing else to split"
        );
        assert_eq!(
            step(
                &FlowFacts {
                    unsigned: Some(txid),
                    both: vec![shared(1), shared(2)],
                    reserved: vec![shared(1)],
                    ..facts()
                },
                Method::SplitKey,
                false
            ),
            Step::Review,
            "an unsent split holds back only its own coins"
        );
        assert_eq!(
            step(
                &FlowFacts {
                    sending: Some(3),
                    unsigned: Some(txid),
                    ..facts()
                },
                Method::SplitKey,
                false
            ),
            Step::Sent { confirmations: 3 },
            "a sent split wins over everything"
        );
        let rest = FlowFacts {
            split_before: 2,
            ..facts()
        };
        assert_eq!(
            step(&rest, Method::SplitKey, false),
            Step::Rest { split: 2 }
        );
        assert_eq!(step(&rest, Method::SplitKey, true), Step::Review);
        let nothing = FlowFacts {
            both: Vec::new(),
            ..facts()
        };
        assert_eq!(step(&nothing, Method::SplitKey, false), Step::Done);
        assert_eq!(
            step(
                &FlowFacts {
                    unverified: 2,
                    ..nothing
                },
                Method::SplitKey,
                false
            ),
            Step::Verifying { count: 2 },
            "done only on positive evidence"
        );
    }

    #[test]
    fn a_vault_with_a_split_coin_starts_at_review() {
        let (panel, cache, [a, b, _]) = fixture();
        let flow = panel.flow(&cache);
        assert_eq!(flow.step, Step::Review);
        assert_eq!(flow.method, Method::SplitKey, "Bitcoin splits with a coin");
        assert_eq!(flow.key, Some(Amount::from_sat(30_000)));
        assert_eq!(
            flow.coins.iter().map(|c| c.outpoint).collect::<Vec<_>>(),
            vec![a, b]
        );
        assert_eq!(flow.copy, CopyCard::Create);
    }

    /// An unavailable fee rate is retried on its own, still "Getting the fee
    /// rate…", before the review says so and offers Try again, which starts
    /// over. A rate clears the failure.
    #[test]
    fn an_unavailable_fee_rate_retries_then_offers_try_again() {
        let (mut panel, cache, _) = fixture();
        for _ in 0..FEE_AUTO_RETRIES {
            let task = panel.update(None, &cache, Message::SplitFeeRate(None));
            assert!(task.units() > 0, "retried on its own");
            assert!(!panel.flow(&cache).fee_failed, "still getting the fee rate");
        }
        let task = panel.update(None, &cache, Message::SplitFeeRate(None));
        assert_eq!(task.units(), 0, "no more automatic retries");
        let flow = panel.flow(&cache);
        assert!(flow.fee_failed);
        assert_eq!(
            flow.blocked(),
            Some("Fee rate unavailable. Try again later.")
        );

        let retry = || Message::View(ViewMessage::Split(SplitMessage::RetryFee));
        let task = panel.update(None, &cache, retry());
        assert!(task.units() > 0, "Try again fetches the fee rate");
        assert!(!panel.flow(&cache).fee_failed);
        // A fresh run of automatic retries.
        let task = panel.update(None, &cache, Message::SplitFeeRate(None));
        assert!(task.units() > 0);

        let _ = panel.update(None, &cache, Message::SplitFeeRate(Some(4)));
        let flow = panel.flow(&cache);
        assert_eq!((flow.feerate, flow.fee_failed), (Some(4), false));
        assert_eq!(
            panel.update(None, &cache, retry()).units(),
            0,
            "nothing to retry with a fee rate"
        );
    }

    #[test]
    fn coins_toggle_and_sign_waits_for_the_fee_then_locks() {
        let (mut panel, cache, [a, b, key]) = fixture();
        let toggle = |o| Message::View(ViewMessage::Split(SplitMessage::ToggleCoin(o)));
        let _ = panel.update(None, &cache, toggle(b));
        let flow = panel.flow(&cache);
        assert_eq!(flow.spent(), (1, Amount::from_sat(100_000)));

        let sign = || Message::View(ViewMessage::Split(SplitMessage::Sign));
        let _ = panel.update(None, &cache, sign());
        assert!(!panel.working, "no fee rate yet");
        let _ = panel.update(None, &cache, Message::SplitFeeRate(Some(4)));
        let task = panel.update(None, &cache, sign());
        assert!(panel.working);
        assert!(task.units() > 0);
        // Locked while building.
        let _ = panel.update(None, &cache, toggle(a));
        assert!(panel.flow(&cache).coins[0].included);
        let _ = key;

        let _ = panel.update(
            None,
            &cache,
            Message::SplitPrepared(Err("Couldn't build".into())),
        );
        assert_eq!(
            panel.split_progress_for_test(),
            (false, Some("Couldn't build".into()))
        );
        assert!(!panel.flow(&cache).coins[1].included, "choices survive");
    }

    #[test]
    fn the_signature_method_needs_a_capable_vault() {
        let (mut panel, cache, _) = fixture();
        let _ = panel.update(
            None,
            &cache,
            Message::View(ViewMessage::Split(SplitMessage::Method(Method::Signature))),
        );
        assert_eq!(panel.flow(&cache).method, Method::SplitKey);
    }

    /// Without a split coin the flow asks for one, and follows the coin
    /// sent to its address: received, checking, or on both chains.
    #[test]
    fn without_a_split_coin_the_flow_follows_the_one_sent() {
        let (mut panel, mut cache, [_, _, key]) = fixture();
        cache.chain_proofs.clear();
        cache.daemon_cache.coins.retain(|coin| coin.outpoint != key);
        assert_eq!(panel.flow(&cache).step, Step::Key(KeyStatus::Waiting));

        const FRESH: &str = "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4";
        let _ = panel.update(None, &cache, Message::SplitAddress(Ok(FRESH.into())));
        assert!(panel.qr.is_some());
        let incoming = shared(7);
        cache.daemon_cache.coins.push(Coin {
            address: bitcoin::Address::from_str(FRESH).unwrap().assume_checked(),
            ..coin(incoming, 25_000, None)
        });
        assert_eq!(
            panel.flow(&cache).step,
            Step::Key(KeyStatus::Confirming {
                amount: Amount::from_sat(25_000)
            })
        );
        cache.daemon_cache.coins.last_mut().unwrap().block_height = Some(110);
        assert_eq!(
            panel.flow(&cache).step,
            Step::Key(KeyStatus::Checking {
                amount: Amount::from_sat(25_000),
                confirmations: 1
            })
        );
        cache.record_entanglement(
            incoming.txid,
            Entanglement::Entangled,
            std::time::Instant::now(),
        );
        let failed = panel.flow(&cache).step;
        assert_eq!(
            failed,
            Step::Key(KeyStatus::BothChains {
                amount: Amount::from_sat(25_000)
            })
        );

        // "Send one from another source": the replacement shows, not the
        // coin that failed, whether it comes to another address or the same.
        let replacement = shared(6);
        cache.daemon_cache.coins.push(Coin {
            address: bitcoin::Address::from_str("bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq")
                .unwrap()
                .assume_checked(),
            ..coin(replacement, 40_000, None)
        });
        assert_eq!(
            panel.flow(&cache).step,
            Step::Key(KeyStatus::Confirming {
                amount: Amount::from_sat(40_000)
            })
        );
        cache.daemon_cache.coins.last_mut().unwrap().address =
            bitcoin::Address::from_str(FRESH).unwrap().assume_checked();
        assert_eq!(
            panel.flow(&cache).step,
            Step::Key(KeyStatus::Confirming {
                amount: Amount::from_sat(40_000)
            })
        );
    }

    /// A built split waits to be signed; once sent, the tab follows its
    /// confirmations; once confirmed with coins left, it offers the rest.
    #[test]
    fn after_signing_the_flow_follows_the_split() {
        let (mut panel, mut cache, [a, b, key]) = fixture();
        let txid = shared(8).txid;
        cache.split_records.push(split_coins::SplitRecord {
            txid,
            chain: ChainId::Bitcoin,
            method: Method::SplitKey,
            inputs: vec![a, b],
            key: Some(key),
            key_path: None,
            created_at: 0,
            confirmations: 0,
        });
        assert_eq!(panel.flow(&cache).step, Step::Unsigned { txid });

        // Spending only `a`, it holds back `a` alone: `b` can still be
        // split, under a notice pointing at the split waiting to be signed.
        cache.split_records[0].inputs = vec![a];
        let flow = panel.flow(&cache);
        assert_eq!(flow.waiting_to_sign, Some(txid));
        assert_eq!(
            flow.coins.iter().map(|c| c.outpoint).collect::<Vec<_>>(),
            vec![b]
        );
        assert!(
            matches!(flow.step, Step::Key(_)),
            "its split coin is held back too"
        );

        let output = OutPoint::new(txid, 0);
        cache.daemon_cache.coins.push(Coin {
            is_change: true,
            ..coin(output, 129_000, Some(109))
        });
        cache.splitting.insert(output, 2);
        assert_eq!(panel.flow(&cache).step, Step::Sent { confirmations: 2 });

        cache.splitting.clear();
        cache.split_records[0].confirmations = SPLIT_CONFIRMATIONS;
        for coin in cache.daemon_cache.coins.iter_mut() {
            if coin.outpoint == a || coin.outpoint == key {
                coin.spend_info = Some(LCSpendInfo {
                    txid,
                    height: Some(109),
                });
            }
        }
        let flow = panel.flow(&cache);
        assert_eq!(flow.step, Step::Rest { split: 1 });
        assert_eq!(flow.coins.len(), 1);
        assert_eq!(flow.coins[0].outpoint, b);
        let _ = panel.update(
            None,
            &cache,
            Message::View(ViewMessage::Split(SplitMessage::SplitRest)),
        );
        // The spent split coin is gone; the split output isn't proven yet.
        assert_eq!(panel.flow(&cache).step, Step::Key(KeyStatus::Waiting));
    }

    /// An imported Vault never sees the signature: it splits with a coin
    /// on both chains, and offers its copy on Bitcoin.
    #[test]
    fn imported_vault_splits_with_a_coin_and_offers_its_copy_on_bitcoin() {
        use crate::services::split_test_wallets::{imported_fork_vault, imported_vault, SHAPES};
        for shape in SHAPES {
            for wallet in [imported_vault(shape), imported_fork_vault(shape)] {
                let here = wallet.chain;
                let panel = SplitHubPanel::new(Arc::new(wallet));
                let flow = panel.flow(&Cache::default());
                assert!(!flow.signature_available, "{:?} {:?}", shape, here);
                assert_eq!(flow.method, Method::SplitKey);
                assert_eq!(
                    flow.copy,
                    if here.is_blake2b() {
                        CopyCard::ThisIsTheCopy
                    } else {
                        CopyCard::Create
                    }
                );
            }
        }
    }

    /// The tab mounts each step without a dialog, and the copy card follows
    /// whether a copy exists.
    #[test]
    fn the_tab_mounts_the_flow() {
        use crate::app::view::shared::landing::capture;
        let (panel, mut cache, _) = fixture();
        let mounted = capture::inspect(
            "vault-split-coins-tab",
            view::vault::split_coins::split_flow_view(&panel.flow(&cache), None),
            (1280, 900),
            "dark",
            coincube_ui::theme::Theme::dark(),
        );
        assert!(mounted.contains("Review the split"));
        assert!(mounted.contains("Cold storage") || mounted.contains("No label"));
        assert!(mounted.contains("Create copy"));
        cache.btcb2_already_claimed = true;
        let mounted = capture::inspect(
            "vault-split-coins-tab-copied",
            view::vault::split_coins::split_flow_view(&panel.flow(&cache), None),
            (1280, 900),
            "dark",
            coincube_ui::theme::Theme::dark(),
        );
        assert!(!mounted.contains("Create copy"));
        assert!(mounted.contains("Switch to it from the Cube menu"));
    }
}

use super::*;
use bitcoin::{
    bip32::{Xpriv, Xpub},
    hashes::Hash,
    secp256k1::Secp256k1,
};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    },
};

#[test]
fn deposit_summary_counts_confirmation_depth_per_output() {
    let confirmed = |value, block_height| Utxo {
        txid: Txid::from_byte_array([1; 32]),
        vout: 0,
        value,
        status: Status {
            confirmed: true,
            block_height: Some(block_height),
            block_hash: Some(BlockHash::from_byte_array([2; 32])),
        },
    };
    let funds =
        summarize_address_funds(&[confirmed(10_000, 100), confirmed(800, 105)], Some(105)).unwrap();
    assert_eq!(funds.confirmed_sats, 10_800);
    assert_eq!(funds.mature_sats, 10_000);
    assert_eq!(funds.largest_confirmed_sats, 10_000);
    assert_eq!(funds.largest_mature_sats, 10_000);
    assert_eq!(funds.min_confirmations, 1);
    let funds = summarize_address_funds(&[confirmed(10_000, 101)], Some(105)).unwrap();
    assert_eq!(funds.largest_mature_sats, 0);
    assert_eq!(funds.mature_sats, 0);
    assert_eq!(funds.min_confirmations, 5);
}

fn ranged() -> String {
    let secp = Secp256k1::new();
    let xpub = Xpub::from_priv(
        &secp,
        &Xpriv::new_master(bitcoin::Network::Bitcoin, &[42; 32]).unwrap(),
    );
    format!("wpkh({}/0/*)", xpub)
}

fn ranged_key(seed: u8) -> String {
    let secp = Secp256k1::new();
    let xpub = Xpub::from_priv(
        &secp,
        &Xpriv::new_master(bitcoin::Network::Bitcoin, &[seed; 32]).unwrap(),
    );
    format!("{xpub}/0/*")
}
fn plan(end: u32) -> ScanPlan {
    ScanPlan {
        chain: ChainId::BitcoinBlake2b,
        branches: vec![BranchRange {
            descriptor: ScanDescriptor::parse(Branch::External, &ranged()).unwrap(),
            start: 0,
            end_exclusive: end,
        }],
        gap: 2,
    }
}
struct Fake {
    used_first: bool,
    spent_addresses: BTreeSet<String>,
    changed: bool,
    anchor_tip_differs: bool,
    anchor_changes_fork_height: bool,
    reorder_second_utxo_read: bool,
    tips: AtomicUsize,
    anchor_reads: AtomicUsize,
    utxo_reads: AtomicUsize,
    coins: Mutex<Vec<Utxo>>,
    previous: Option<Transaction>,
}
impl Fake {
    fn empty() -> Self {
        Self {
            used_first: false,
            spent_addresses: BTreeSet::new(),
            changed: false,
            anchor_tip_differs: false,
            anchor_changes_fork_height: false,
            reorder_second_utxo_read: false,
            tips: AtomicUsize::new(0),
            anchor_reads: AtomicUsize::new(0),
            utxo_reads: AtomicUsize::new(0),
            coins: Mutex::new(vec![]),
            previous: None,
        }
    }
}
#[async_trait]
impl Source for Fake {
    async fn tip(&self, _: ChainId) -> Result<BlockHash, ScanError> {
        let n = self.tips.fetch_add(1, Ordering::SeqCst);
        Ok(BlockHash::from_byte_array(
            [if self.changed && n > 0 { 2 } else { 1 }; 32],
        ))
    }
    async fn tip_height(&self, _: ChainId, tip: BlockHash) -> Result<u32, ScanError> {
        Ok(u32::from(tip.to_byte_array()[0]) + 800_000)
    }
    async fn anchor(&self) -> Result<(BlockHash, Option<u64>), ScanError> {
        let read = self.anchor_reads.fetch_add(1, Ordering::SeqCst);
        Ok((
            BlockHash::from_byte_array([if self.anchor_tip_differs { 2 } else { 1 }; 32]),
            Some(if self.anchor_changes_fork_height && read > 0 {
                101
            } else {
                100
            }),
        ))
    }
    async fn stats(&self, _: ChainId, address: &str) -> Result<Stats, ScanError> {
        let first = bitcoin::Address::from_script(
            &ScanDescriptor::parse(Branch::External, &ranged())
                .unwrap()
                .script(0)
                .unwrap(),
            bitcoin::Network::Bitcoin,
        )
        .unwrap()
        .to_string();
        Ok(Stats {
            chain_stats: Count {
                tx_count: u64::from(
                    (self.used_first && address == first)
                        || self.previous.is_some()
                        || self.spent_addresses.contains(address),
                ),
            },
            mempool_stats: Count { tx_count: 0 },
        })
    }
    async fn utxos(&self, _: ChainId, _: &str) -> Result<Vec<Utxo>, ScanError> {
        let mut coins = self.coins.lock().unwrap().clone();
        if self.reorder_second_utxo_read && self.utxo_reads.fetch_add(1, Ordering::SeqCst) == 1 {
            coins.reverse();
        }
        Ok(coins)
    }
    async fn transaction(&self, _: ChainId, _: Txid) -> Result<Transaction, ScanError> {
        self.previous.clone().ok_or(ScanError::Unavailable)
    }
}

#[tokio::test]
async fn reordered_utxos_with_identical_records_do_not_change_scan() {
    let mut p = plan(1);
    p.branches[0].descriptor =
        ScanDescriptor::parse(Branch::External, &ranged().replace("/*", "/0")).unwrap();
    p.gap = 1;
    let script = p.branches[0].descriptor.script(0).unwrap();
    let previous = Transaction {
        version: bitcoin::transaction::Version::TWO,
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![],
        output: vec![
            bitcoin::TxOut {
                value: bitcoin::Amount::from_sat(1000),
                script_pubkey: script.clone(),
            },
            bitcoin::TxOut {
                value: bitcoin::Amount::from_sat(2000),
                script_pubkey: script,
            },
        ],
    };
    let txid = previous.compute_txid();
    let mut source = Fake::empty();
    source.previous = Some(previous);
    source.reorder_second_utxo_read = true;
    *source.coins.lock().unwrap() = (0..2)
        .map(|vout| Utxo {
            txid,
            vout,
            value: 1000 + 1000 * u64::from(vout),
            status: Status {
                confirmed: true,
                block_height: Some(100),
                block_hash: Some(BlockHash::from_byte_array([5; 32])),
            },
        })
        .collect();
    for chain in [ChainId::BitcoinBlake2b, ChainId::Bitcoin] {
        p.chain = chain;
        source.utxo_reads.store(0, Ordering::SeqCst);
        let report = collect(&source, &p, 7).await.unwrap();
        assert_eq!(report.coins().len(), 2);
        assert_eq!(report.coins()[0].outpoint.vout, 0);
        assert_eq!(report.coins()[1].outpoint.vout, 1);
    }
}

#[test]
fn utxo_comparison_rejects_changed_records_and_duplicates() {
    let coin = Utxo {
        txid: Txid::from_byte_array([1; 32]),
        vout: 0,
        value: 1000,
        status: Status {
            confirmed: true,
            block_height: Some(100),
            block_hash: Some(BlockHash::from_byte_array([2; 32])),
        },
    };
    let mut other = coin.clone();
    other.value += 1;
    assert_eq!(same_utxos(std::slice::from_ref(&coin), &[other]), Ok(false));
    let mut other = coin.clone();
    other.status.block_height = Some(101);
    assert_eq!(same_utxos(std::slice::from_ref(&coin), &[other]), Ok(false));
    let mut other = coin.clone();
    other.vout = 1;
    assert_eq!(same_utxos(std::slice::from_ref(&coin), &[other]), Ok(false));
    assert_eq!(
        same_utxos(&[coin.clone(), coin.clone()], std::slice::from_ref(&coin)),
        Err(ScanError::Malformed)
    );
    assert_eq!(
        same_utxos(std::slice::from_ref(&coin), &[coin.clone(), coin.clone()]),
        Err(ScanError::Malformed)
    );
}
#[tokio::test]
async fn spent_history_is_not_an_unused_address_and_limits_are_incomplete() {
    let mut source = Fake::empty();
    source.used_first = true;
    let report = collect(&source, &plan(4), 7).await.unwrap();
    assert_eq!(report.addresses_scanned(), 3);
    assert!(report.coins().is_empty());
    assert_eq!(report.generation(), 7);
    // Coverage records the exact proven walk: index 0 had history, 1..3 did not.
    assert_eq!(
        report.coverage(Branch::External),
        Some(BranchCoverage {
            branch: Branch::External,
            start: 0,
            end_exclusive: 3,
            last_used: Some(0),
        })
    );
    assert_eq!(report.coverage(Branch::Internal), None);
    let mut source = Fake::empty();
    source.used_first = true;
    assert!(matches!(
        collect(&source, &plan(2), 7).await,
        Err(ScanError::RangeLimit)
    ));
    let mut source = Fake::empty();
    source.changed = true;
    assert!(matches!(
        collect(&source, &plan(4), 7).await,
        Err(ScanError::Changed(ChangedReason::TipMoved))
    ));
    let mut source = Fake::empty();
    source.anchor_tip_differs = true;
    assert!(matches!(
        collect(&source, &plan(4), 7).await,
        Err(ScanError::Changed(ChangedReason::AnchorMismatch))
    ));
    let mut source = Fake::empty();
    source.anchor_changes_fork_height = true;
    assert!(matches!(
        collect(&source, &plan(4), 7).await,
        Err(ScanError::Changed(ChangedReason::AnchorChanged))
    ));
}

fn imported_plan(end: u32) -> ScanPlan {
    let external = ScanDescriptor::parse(Branch::External, &ranged()).unwrap();
    let internal =
        ScanDescriptor::parse(Branch::Internal, &ranged().replace("/0/*", "/1/*")).unwrap();
    ScanPlan {
        chain: ChainId::BitcoinBlake2b,
        branches: vec![
            BranchRange {
                descriptor: external,
                start: 0,
                end_exclusive: end,
            },
            BranchRange {
                descriptor: internal,
                start: 0,
                end_exclusive: end,
            },
        ],
        gap: 2,
    }
}

struct LateHistory {
    coins: BTreeMap<String, (Utxo, Transaction)>,
}

impl LateHistory {
    fn new(plan: &ScanPlan) -> Self {
        use bitcoin::{absolute, transaction, Amount, TxIn, TxOut};
        let mut coins = BTreeMap::new();
        for (offset, branch, index) in [(1, Branch::External, 25), (2, Branch::Internal, 27)] {
            let descriptor = &plan
                .branches
                .iter()
                .find(|range| range.descriptor.branch() == branch)
                .unwrap()
                .descriptor;
            let script = descriptor.script(index).unwrap();
            let address = bitcoin::Address::from_script(&script, bitcoin::Network::Bitcoin)
                .unwrap()
                .to_string();
            let previous = Transaction {
                version: transaction::Version::TWO,
                lock_time: absolute::LockTime::from_consensus(offset),
                input: vec![TxIn::default()],
                output: vec![TxOut {
                    value: Amount::from_sat(10_000 + u64::from(offset)),
                    script_pubkey: script,
                }],
            };
            coins.insert(
                address,
                (
                    Utxo {
                        txid: previous.compute_txid(),
                        vout: 0,
                        value: 10_000 + u64::from(offset),
                        status: Status {
                            confirmed: true,
                            block_height: Some(90),
                            block_hash: Some(BlockHash::from_byte_array([3; 32])),
                        },
                    },
                    previous,
                ),
            );
        }
        Self { coins }
    }
}

#[async_trait]
impl Source for LateHistory {
    async fn tip(&self, _: ChainId) -> Result<BlockHash, ScanError> {
        Ok(BlockHash::from_byte_array([1; 32]))
    }
    async fn tip_height(&self, _: ChainId, _: BlockHash) -> Result<u32, ScanError> {
        Ok(800_000)
    }
    async fn anchor(&self) -> Result<(BlockHash, Option<u64>), ScanError> {
        Ok((BlockHash::from_byte_array([1; 32]), Some(100)))
    }
    async fn stats(&self, _: ChainId, address: &str) -> Result<Stats, ScanError> {
        Ok(Stats {
            chain_stats: Count {
                tx_count: u64::from(self.coins.contains_key(address)),
            },
            mempool_stats: Count { tx_count: 0 },
        })
    }
    async fn utxos(&self, _: ChainId, address: &str) -> Result<Vec<Utxo>, ScanError> {
        Ok(self
            .coins
            .get(address)
            .map(|(coin, _)| vec![coin.clone()])
            .unwrap_or_default())
    }
    async fn transaction(&self, _: ChainId, txid: Txid) -> Result<Transaction, ScanError> {
        self.coins
            .values()
            .find(|(_, tx)| tx.compute_txid() == txid)
            .map(|(_, tx)| tx.clone())
            .ok_or(ScanError::Prevout)
    }
}

#[tokio::test]
async fn imported_full_window_finds_both_branches_after_an_early_gap() {
    let plan = imported_plan(40);
    let source = LateHistory::new(&plan);
    let ordinary = collect(&source, &plan, 7).await.unwrap();
    assert_eq!(ordinary.addresses_scanned(), 4);
    assert!(ordinary.coins().is_empty());
    let full = collect_mode(&source, &plan, 7, ScanMode::FullImported)
        .await
        .unwrap();
    assert_eq!(full.addresses_scanned(), 80);
    assert_eq!(
        full.coins()
            .iter()
            .map(|coin| (coin.branch, coin.index))
            .collect::<Vec<_>>(),
        [(Branch::External, 25), (Branch::Internal, 27)]
    );
    for branch in [Branch::External, Branch::Internal] {
        assert_eq!(full.coverage(branch).unwrap().end_exclusive, 40);
    }
}

struct SpentEverywhere;

#[async_trait]
impl Source for SpentEverywhere {
    async fn tip(&self, _: ChainId) -> Result<BlockHash, ScanError> {
        Ok(BlockHash::from_byte_array([1; 32]))
    }
    async fn tip_height(&self, _: ChainId, _: BlockHash) -> Result<u32, ScanError> {
        Ok(800_000)
    }
    async fn anchor(&self) -> Result<(BlockHash, Option<u64>), ScanError> {
        Ok((BlockHash::from_byte_array([1; 32]), Some(100)))
    }
    async fn stats(&self, _: ChainId, _: &str) -> Result<Stats, ScanError> {
        Ok(Stats {
            chain_stats: Count { tx_count: 1 },
            mempool_stats: Count { tx_count: 0 },
        })
    }
    async fn utxos(&self, _: ChainId, _: &str) -> Result<Vec<Utxo>, ScanError> {
        Ok(Vec::new())
    }
    async fn transaction(&self, _: ChainId, _: Txid) -> Result<Transaction, ScanError> {
        panic!("spent history has no unspent transaction")
    }
}

#[tokio::test]
async fn ordinary_scanner_keeps_its_200_address_budget() {
    assert!(matches!(
        collect(&SpentEverywhere, &plan(MAX_ADDRESSES + 1), 7).await,
        Err(ScanError::AddressLimit)
    ));
}

struct OverlappingReads {
    active: AtomicUsize,
    peak: AtomicUsize,
    stats: AtomicUsize,
    utxos: AtomicUsize,
}

impl OverlappingReads {
    async fn wait(&self) {
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(active, Ordering::SeqCst);
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        self.active.fetch_sub(1, Ordering::SeqCst);
    }
}

#[async_trait]
impl Source for OverlappingReads {
    async fn tip(&self, _: ChainId) -> Result<BlockHash, ScanError> {
        Ok(BlockHash::from_byte_array([1; 32]))
    }
    async fn tip_height(&self, _: ChainId, _: BlockHash) -> Result<u32, ScanError> {
        Ok(800_000)
    }
    async fn anchor(&self) -> Result<(BlockHash, Option<u64>), ScanError> {
        Ok((BlockHash::from_byte_array([1; 32]), Some(100)))
    }
    async fn stats(&self, _: ChainId, _: &str) -> Result<Stats, ScanError> {
        self.stats.fetch_add(1, Ordering::SeqCst);
        self.wait().await;
        Ok(Stats {
            chain_stats: Count { tx_count: 0 },
            mempool_stats: Count { tx_count: 0 },
        })
    }
    async fn utxos(&self, _: ChainId, _: &str) -> Result<Vec<Utxo>, ScanError> {
        self.utxos.fetch_add(1, Ordering::SeqCst);
        self.wait().await;
        Ok(Vec::new())
    }
    async fn transaction(&self, _: ChainId, _: Txid) -> Result<Transaction, ScanError> {
        panic!("an empty address must not fetch a transaction")
    }
}

#[tokio::test]
async fn address_window_overlaps_reads_without_exceeding_gap_or_request_bound() {
    let source = OverlappingReads {
        active: AtomicUsize::new(0),
        peak: AtomicUsize::new(0),
        stats: AtomicUsize::new(0),
        utxos: AtomicUsize::new(0),
    };
    let mut plan = plan(20);
    plan.gap = 8;
    let report = collect(&source, &plan, 7).await.unwrap();
    assert_eq!(report.addresses_scanned(), 8);
    assert_eq!(source.stats.load(Ordering::SeqCst), 16);
    assert_eq!(source.utxos.load(Ordering::SeqCst), 16);
    assert!(source.peak.load(Ordering::SeqCst) > 2);
    assert!(source.peak.load(Ordering::SeqCst) <= 2 * ADDRESS_BATCH as usize);
}
#[test]
fn descriptor_capabilities_are_scan_only_and_ambiguous_or_secret_paths_refuse() {
    let text = ranged();
    let public = text.trim_start_matches("wpkh(").trim_end_matches(')');
    let key_a = ranged_key(41);
    let key_b = ranged_key(42);
    let key_c = ranged_key(43);
    let origin = format!("[abcd1234/84h/0h/0h]{public}");
    // (descriptor, PSBT-file route, in-app routes): the in-app routes need
    // an origin on every key (U6).
    for (text, psbt_file, matched) in [
        (text.clone(), true, false),
        (format!("pkh({})", public), true, false),
        (format!("wpkh({origin})"), true, true),
        (format!("sh(wpkh({}))", public), true, false),
        // `tr` is scan-only: no route, not even a PSBT file.
        (format!("tr({})", public), false, false),
        (format!("tr([abcd1234/86h/0h/0h]{public})"), false, false),
        (
            format!("wsh(multi(2,{key_a},{key_b},{key_c}))"),
            true,
            false,
        ),
        (
            format!("wsh(sortedmulti(2,{key_a},{key_b},{key_c}))"),
            true,
            false,
        ),
    ] {
        let d = ScanDescriptor::parse(Branch::External, &text).unwrap();
        assert_eq!(d.end_exclusive(100), 100);
        assert_eq!(
            d.capabilities(),
            Capabilities {
                scan: true,
                signing: SigningRoutes {
                    psbt_file,
                    in_app_hardware: matched,
                    seed_unified: matched,
                },
                claim_authorization: false
            },
            "{text}"
        );
        assert_eq!(d.is_taproot(), !psbt_file);
    }
    for text in [
        text.replace("/0/*", "/<0;1>/*"),
        text.replace("/0/*", "/0'/*"),
        text.replace("/0/*", "/0/*'"),
        "raw(51)".into(),
        format!("wsh(pk({public}))"),
        format!("wsh(and_v(v:pk({public}),older(10)))"),
        format!("tr({public},pk({public}))"),
        "wpkh(not-a-key)".into(),
    ] {
        assert!(ScanDescriptor::parse(Branch::External, &text).is_err());
    }
    let secret = Xpriv::new_master(bitcoin::Network::Bitcoin, &[42; 32]).unwrap();
    assert!(ScanDescriptor::parse(Branch::External, &format!("wpkh({}/0/*)", secret)).is_err());
    let mut p = plan(3);
    p.chain = ChainId::BitcoinBlake2bTestnet4;
    assert_eq!(p.validate(), Err(ScanError::UnsupportedChain));
}

/// #568 criterion 3: every accepted shape parses with and without key
/// origins, and the origin is a label only (same scripts, same routes). The
/// refusals hold for every shape, not only `wpkh`.
#[test]
fn descriptor_matrix_covers_every_shape_with_and_without_origins() {
    let secp = Secp256k1::new();
    let xpriv = |seed: u8| Xpriv::new_master(bitcoin::Network::Bitcoin, &[seed; 32]).unwrap();
    let xpub = |seed: u8| Xpub::from_priv(&secp, &xpriv(seed));
    let single =
        |seed: u8| bitcoin::PublicKey::new(xpriv(seed).private_key.public_key(&secp)).to_string();
    // Key expressions for one or three keys, with a caller-chosen suffix and an
    // optional origin per key. Hardened origins are labels; only a hardened
    // derivation *suffix* is refused.
    let key = |seed: u8, origin: Option<&str>, suffix: &str| match origin {
        Some(origin) => format!("[{origin}]{}{suffix}", xpub(seed)),
        None => format!("{}{suffix}", xpub(seed)),
    };
    type Shape = fn(&[String]) -> String;
    let shapes: [(&str, Shape, usize, bool); 6] = [
        ("wpkh", |k| format!("wpkh({})", k[0]), 1, true),
        ("sh(wpkh)", |k| format!("sh(wpkh({}))", k[0]), 1, true),
        ("pkh", |k| format!("pkh({})", k[0]), 1, true),
        (
            "wsh(multi)",
            |k| format!("wsh(multi(2,{},{},{}))", k[0], k[1], k[2]),
            3,
            true,
        ),
        (
            "wsh(sortedmulti)",
            |k| format!("wsh(sortedmulti(2,{},{},{}))", k[0], k[1], k[2]),
            3,
            true,
        ),
        // Scan-only: no signing route of any kind, with or without origin.
        ("tr", |k| format!("tr({})", k[0]), 1, false),
    ];
    let origins = [
        "d34db33f/84h/0h/0h",
        "0badc0de/48h/0h/0h/2h",
        "cafef00d/48'/0'/0'/2'",
    ];
    let keys = |count: usize, with_origin: bool, suffix: &str| -> Vec<String> {
        (0..count)
            .map(|i| key(50 + i as u8, with_origin.then_some(origins[i]), suffix))
            .collect()
    };
    // The in-app routes (device, unified seed) need an origin on every key
    // of a non-`tr` shape (U6); without one they fail closed.
    let routes = |psbt_file, matched| Capabilities {
        scan: true,
        signing: SigningRoutes {
            psbt_file,
            in_app_hardware: matched,
            seed_unified: matched,
        },
        claim_authorization: false,
    };

    for (name, shape, count, psbt_file) in shapes {
        let bare = ScanDescriptor::parse(Branch::External, &shape(&keys(count, false, "/0/*")))
            .unwrap_or_else(|e| panic!("{} without origin: {:?}", name, e));
        let with_origin =
            ScanDescriptor::parse(Branch::Internal, &shape(&keys(count, true, "/1/*")))
                .unwrap_or_else(|e| panic!("{} with origin: {:?}", name, e));
        let same_branch_origin =
            ScanDescriptor::parse(Branch::External, &shape(&keys(count, true, "/0/*"))).unwrap();
        assert_eq!(bare.capabilities(), routes(psbt_file, false), "{}", name);
        for d in [&with_origin, &same_branch_origin] {
            assert_eq!(d.capabilities(), routes(psbt_file, psbt_file), "{}", name);
        }
        for d in [&bare, &with_origin, &same_branch_origin] {
            assert_eq!(d.is_taproot(), name == "tr", "{}", name);
            assert!(d.is_ranged(), "{}", name);
            assert_eq!(d.end_exclusive(100), 100, "{}", name);
        }
        assert_eq!(with_origin.branch(), Branch::Internal);
        // The origin is kept for the signer but never changes an address.
        assert!(
            same_branch_origin.canonical().contains("[d34db33f/"),
            "{}",
            name
        );
        for index in [0, 1, 99] {
            assert_eq!(
                bare.script(index).unwrap(),
                same_branch_origin.script(index).unwrap(),
                "{name} at {index}"
            );
        }
        // Mixed: only some multisig keys carry an origin.
        if count == 3 {
            let mut mixed = keys(count, true, "/0/*");
            mixed[1] = key(51, None, "/0/*");
            let d = ScanDescriptor::parse(Branch::External, &shape(&mixed)).unwrap();
            assert_eq!(d.script(5).unwrap(), bare.script(5).unwrap(), "{}", name);
            // One key without its origin: no in-app route.
            assert_eq!(d.capabilities(), routes(psbt_file, false), "{}", name);
        }

        check_refused_shapes(name, shape, count, &origins, keys, xpriv, &secp);
    }

    // Single (non-extended) public keys are fixed, one-address descriptors,
    // with or without an origin; a compressed key is required by segwit.
    for (text, psbt_file) in [
        (format!("wpkh({})", single(70)), true),
        (
            format!("wpkh([d34db33f/84h/0h/0h/0/5]{})", single(70)),
            true,
        ),
        (
            format!("sh(wpkh([d34db33f/49h/0h/0h/0/5]{}))", single(70)),
            true,
        ),
        (format!("pkh([d34db33f/44h/0h/0h/0/5]{})", single(70)), true),
        (
            format!(
                "wsh(sortedmulti(2,[d34db33f/48h/0h/0h/2h/0/0]{},{},{}))",
                single(71),
                single(72),
                single(73)
            ),
            true,
        ),
    ] {
        let d = ScanDescriptor::parse(Branch::External, &text).unwrap();
        assert!(!d.is_ranged(), "{}", text);
        assert_eq!(d.end_exclusive(100), 1, "{}", text);
        // A fixed key has no in-app route, with or without an origin (P7).
        assert_eq!(d.capabilities(), routes(psbt_file, false), "{}", text);
    }

    // Unsupported forms fail closed with or without origins.
    let o = origins[0];
    let k = xpub(80);
    for text in [
        format!("tr([{o}]{k}/0/*,pk([{o}]{k}/1/*))"),
        format!("wsh(pk([{o}]{k}/0/*))"),
        format!("sh(wsh(multi(1,[{o}]{k}/0/*)))"),
        format!("sh(multi(1,[{o}]{k}/0/*))"),
        format!("sh(sortedmulti(1,{k}/0/*))"),
        format!("pk([{o}]{k}/0/*)"),
        format!("combo([{o}]{k}/0/*)"),
        "addr(bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq)".to_string(),
        // Malformed origin fingerprint.
        format!("wpkh([d34db3/84h/0h/0h]{k}/0/*)"),
    ] {
        assert_eq!(
            ScanDescriptor::parse(Branch::External, &text).unwrap_err(),
            ScanError::Descriptor,
            "{}",
            text
        );
    }
}

#[test]
fn fixed_descriptor_uses_the_scanners_single_index_range() {
    let fixed = ScanDescriptor::parse(Branch::External, &ranged().replace("/*", "/0")).unwrap();
    assert_eq!(fixed.end_exclusive(100), 1);
}
#[tokio::test]
async fn full_prevout_binding_and_duplicate_outpoints_refuse() {
    let mut p = plan(1);
    p.branches[0].descriptor =
        ScanDescriptor::parse(Branch::External, &ranged().replace("/*", "/0")).unwrap();
    p.gap = 1;
    let script = p.branches[0].descriptor.script(0).unwrap();
    let previous = Transaction {
        version: bitcoin::transaction::Version::TWO,
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![],
        output: vec![bitcoin::TxOut {
            value: bitcoin::Amount::from_sat(1000),
            script_pubkey: script,
        }],
    };
    let coin = Utxo {
        txid: previous.compute_txid(),
        vout: 0,
        value: 1000,
        status: Status {
            confirmed: false,
            block_height: None,
            block_hash: None,
        },
    };
    let mut source = Fake::empty();
    source.previous = Some(previous.clone());
    *source.coins.lock().unwrap() = vec![coin.clone()];
    let report = collect(&source, &p, 0).await.unwrap();
    assert_eq!(report.coins()[0].previous, previous);
    assert_eq!(report.fork_side(&report.coins()[0]), ForkSide::Unconfirmed);
    source.coins.lock().unwrap()[0].value = 1001;
    assert!(matches!(
        collect(&source, &p, 0).await,
        Err(ScanError::Prevout)
    ));
    *source.coins.lock().unwrap() = vec![coin.clone(), coin];
    assert!(matches!(
        collect(&source, &p, 0).await,
        Err(ScanError::Malformed)
    ));
}
#[tokio::test]
async fn cancelled_generation_refuses_before_any_request() {
    let (tx, rx) = watch::channel(2);
    let client = CoincubeClient::for_test("http://127.0.0.1:1".to_owned());
    assert!(matches!(
        scan(client, plan(3), 1, rx).await,
        Err(ScanError::Cancelled)
    ));
    drop(tx);
}

#[tokio::test]
async fn http_fresh_address_privacy_no_fallback_and_failure_matrix() {
    use httpmock::prelude::*;
    for (status, markers, expected) in [
        (503, true, ScanError::Http(503)),
        (200, false, ScanError::Freshness),
        (200, true, ScanError::Malformed),
    ] {
        let server = MockServer::start();
        let mut client = CoincubeClient::for_test(server.base_url());
        client.set_token("synthetic-scan-token");
        let source = http::HttpSource::new(client).unwrap();
        let mock = server.mock(|when, then| {
            when.method(GET)
                .path("/api/v1/esplora/bitcoin-blake2b/mainnet/address/abc")
                .header("x-coincube-observation", "fresh")
                .matches(|r| {
                    r.headers.as_ref().is_none_or(|hs| {
                        hs.iter().all(|(n, _)| {
                            ![
                                "authorization",
                                "cookie",
                                "x-device-fingerprint",
                                "x-device-name",
                            ]
                            .iter()
                            .any(|bad| n.eq_ignore_ascii_case(bad))
                        })
                    })
                });
            let then = then.status(status).body("malformed");
            if markers {
                then.header("X-Coincube-Observation", "fresh")
                    .header("X-Cache", "BYPASS")
                    .header("Cache-Control", "no-store");
            }
        });
        assert_eq!(
            source.stats(ChainId::BitcoinBlake2b, "abc").await,
            Err(expected)
        );
        mock.assert();
    }
}
#[tokio::test]
async fn http_complete_bounded_scan_and_inflight_generation_cancel() {
    use httpmock::prelude::*;
    let server = MockServer::start();
    let p = plan(3);
    let tip = server.mock(|when, then| {
        when.method(GET)
            .path("/api/v1/esplora/bitcoin/mainnet/blocks/tip/hash");
        then.status(200)
            .header("X-Coincube-Observation", "fresh")
            .header("X-Cache", "BYPASS")
            .header("Cache-Control", "no-store")
            .body("11".repeat(32));
    });
    let status = server.mock(|when, then| {
        when.method(GET)
            .path(format!(
                "/api/v1/esplora/bitcoin/mainnet/block/{}/status",
                "11".repeat(32)
            ))
            .header("x-coincube-observation", "fresh");
        then.status(200)
            .header("X-Coincube-Observation", "fresh")
            .header("X-Cache", "BYPASS")
            .header("Cache-Control", "no-store")
            .body(r#"{"in_best_chain":true,"height":912345,"next_best":null}"#);
    });
    let mut mocks = Vec::new();
    for index in 0..2 {
        let addr = bitcoin::Address::from_script(
            &p.branches[0].descriptor.script(index).unwrap(),
            bitcoin::Network::Bitcoin,
        )
        .unwrap()
        .to_string();
        for (suffix, body) in [
            (
                "",
                r#"{"chain_stats":{"tx_count":0},"mempool_stats":{"tx_count":0}}"#,
            ),
            ("/utxo", "[]"),
        ] {
            mocks.push(server.mock(|when, then| {
                when.method(GET).path(format!(
                    "/api/v1/esplora/bitcoin/mainnet/address/{}{}",
                    addr, suffix
                ));
                then.status(200)
                    .header("X-Coincube-Observation", "fresh")
                    .header("X-Cache", "BYPASS")
                    .header("Cache-Control", "no-store")
                    .body(body);
            }));
        }
    }
    let mut client = CoincubeClient::for_test(server.base_url());
    client.set_token("synthetic");
    let (tx, rx) = watch::channel(1);
    let mut p = p;
    p.chain = ChainId::Bitcoin;
    let result = scan(client.clone(), p, 1, rx).await.unwrap();
    assert_eq!(result.addresses_scanned(), 2);
    assert_eq!(result.tip_height(), 912_345);
    tip.assert_hits(2);
    status.assert_hits(1);
    for m in mocks {
        m.assert_hits(2);
    }
    // Cancel an actively polled network operation, not an unpolled future.
    let delayed = server.mock(|when, then| {
        when.method(GET)
            .path("/api/v1/esplora/bitcoin-blake2b/mainnet/blocks/tip/hash");
        then.status(200)
            .delay(Duration::from_secs(10))
            .body("11".repeat(32));
    });
    let task = tokio::spawn(scan(client, plan(3), 1, tx.subscribe()));
    for _ in 0..100 {
        if delayed.hits() > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(delayed.hits(), 1);
    tx.send(2).unwrap();
    assert!(matches!(task.await.unwrap(), Err(ScanError::Cancelled)));
}

#[tokio::test]
async fn response_limits_and_redirects_refuse_without_following() {
    use httpmock::prelude::*;
    let server = MockServer::start();
    let mut client = CoincubeClient::for_test(server.base_url());
    client.set_token("synthetic");
    let source = http::HttpSource::new(client).unwrap();
    let oversized = server.mock(|when, then| {
        when.method(GET)
            .path("/api/v1/esplora/bitcoin/mainnet/address/large");
        then.status(200)
            .header("X-Coincube-Observation", "fresh")
            .header("X-Cache", "BYPASS")
            .header("Cache-Control", "no-store")
            .body(" ".repeat(2 * 1024 * 1024 + 1));
    });
    assert_eq!(
        source.stats(ChainId::Bitcoin, "large").await,
        Err(ScanError::BodyLimit)
    );
    oversized.assert();
    let target = server.mock(|when, then| {
        when.method(GET).path("/leak");
        then.status(200);
    });
    let redirect = server.mock(|when, then| {
        when.method(GET)
            .path("/api/v1/esplora/bitcoin/mainnet/address/redirect");
        then.status(302)
            .header("Location", format!("{}/leak", server.base_url()));
    });
    assert_eq!(
        source.stats(ChainId::Bitcoin, "redirect").await,
        Err(ScanError::Http(302))
    );
    redirect.assert();
    target.assert_hits(0);
}

#[tokio::test]
async fn confirming_height_and_observed_fork_height_classify_coins() {
    let mut p = plan(1);
    p.branches[0].descriptor =
        ScanDescriptor::parse(Branch::External, &ranged().replace("/*", "/0")).unwrap();
    p.gap = 1;
    let previous = Transaction {
        version: bitcoin::transaction::Version::TWO,
        lock_time: bitcoin::absolute::LockTime::ZERO,
        input: vec![],
        output: vec![bitcoin::TxOut {
            value: bitcoin::Amount::from_sat(1000),
            script_pubkey: p.branches[0].descriptor.script(0).unwrap(),
        }],
    };
    let mut source = Fake::empty();
    source.previous = Some(previous.clone());
    let hash = BlockHash::from_byte_array([5; 32]);
    // The fake anchor reports fork height 100: 99 is pre-fork, 100 is not.
    for (height, side) in [(99, ForkSide::PreFork), (100, ForkSide::PostFork)] {
        *source.coins.lock().unwrap() = vec![Utxo {
            txid: previous.compute_txid(),
            vout: 0,
            value: 1000,
            status: Status {
                confirmed: true,
                block_height: Some(height),
                block_hash: Some(hash),
            },
        }];
        let report = collect(&source, &p, 0).await.unwrap();
        assert_eq!(report.fork_height(), Some(100));
        let coin = &report.coins()[0];
        assert_eq!(
            (coin.block_height, coin.block_hash),
            (Some(height), Some(hash))
        );
        assert_eq!(report.fork_side(coin), side);
        // Without an observed fork height nothing is classifiable.
        let unknown = report.clone().with_fork_height(None);
        assert_eq!(unknown.fork_side(coin), ForkSide::Unknown);
    }
}

/// The report's tip height is read for the scan's own tip, and a tip that has
/// left the best chain, or a status without a height, fails the scan.
#[tokio::test]
async fn split_tip_height_is_bound_to_the_scanned_tip() {
    let report = collect(&Fake::empty(), &plan(4), 7).await.unwrap();
    assert_eq!(report.tip(), BlockHash::from_byte_array([1; 32]));
    assert_eq!(report.tip_height(), 800_001);

    use httpmock::prelude::*;
    for (body, expected) in [
        (
            r#"{"in_best_chain":false,"height":5,"next_best":null}"#,
            Err(ScanError::Changed(ChangedReason::TipNoLongerBest)),
        ),
        (r#"{"in_best_chain":true}"#, Err(ScanError::Malformed)),
        (r#"{"in_best_chain":true,"height":7}"#, Ok(7)),
    ] {
        let server = MockServer::start();
        let tip = BlockHash::from_byte_array([9; 32]);
        let mock = server.mock(|when, then| {
            when.method(GET).path(format!(
                "/api/v1/esplora/bitcoin-blake2b/mainnet/block/{tip}/status"
            ));
            then.status(200)
                .header("X-Coincube-Observation", "fresh")
                .header("X-Cache", "BYPASS")
                .header("Cache-Control", "no-store")
                .body(body);
        });
        let mut client = CoincubeClient::for_test(server.base_url());
        client.set_token("synthetic");
        let source = http::HttpSource::new(client).unwrap();
        assert_eq!(
            source.tip_height(ChainId::BitcoinBlake2b, tip).await,
            expected
        );
        mock.assert();
    }
}

#[tokio::test]
async fn rate_limit_cooldown_is_shared_across_sources_and_both_chains() {
    use httpmock::prelude::*;
    let server = MockServer::start();
    let _cooldown = crate::services::esplora_backoff::TestCooldownGuard::new(&server.base_url());
    let mut client = CoincubeClient::for_test(server.base_url());
    client.set_token("synthetic-scan-token");
    let limited = server.mock(|when, then| {
        when.method(GET)
            .path("/api/v1/esplora/bitcoin/mainnet/address/abc");
        then.status(429).header("Retry-After", "3090");
    });
    let mirror = server.mock(|when, then| {
        when.method(GET)
            .path("/api/v1/esplora/bitcoin-blake2b/mainnet/address/abc");
        then.status(200);
    });
    let source = http::HttpSource::new(client.clone()).unwrap();
    assert!(matches!(source.stats(ChainId::Bitcoin, "abc").await,
        Err(ScanError::RateLimited(delay)) if delay.as_secs() == 3090));
    let another = http::HttpSource::new(client.clone()).unwrap();
    assert!(
        matches!(another.stats(ChainId::BitcoinBlake2b, "abc").await,
        Err(ScanError::RateLimited(delay)) if delay > Duration::from_secs(3000))
    );
    let (_sender, receiver) = watch::channel(7);
    let observations = crate::services::claim_observation::http::HttpObservationSource::new(
        client,
        ChainId::Bitcoin,
        ChainId::BitcoinBlake2b,
        crate::services::claim_observation::CollectionContext {
            expected_generation: 7,
            generation: receiver,
        },
    )
    .unwrap();
    use crate::services::claim_observation::{FailureKind, ObservationSource};
    assert!(matches!(
        observations.tip(ChainId::Bitcoin).await,
        Err(FailureKind::Http(429))
    ));
    limited.assert_hits(1);
    // The blocked mirror-chain read must not reach the server either.
    mirror.assert_hits(0);
}

fn check_refused_shapes(
    name: &str,
    shape: fn(&[String]) -> String,
    count: usize,
    origins: &[&str; 3],
    keys: impl Fn(usize, bool, &str) -> Vec<String>,
    xpriv: impl Fn(u8) -> Xpriv,
    secp: &Secp256k1<bitcoin::secp256k1::All>,
) {
    // Refusals, each with and without origins.
    for with_origin in [false, true] {
        let origin_of = |i: usize| with_origin.then_some(origins[i]);
        let refused = [
            // Hardened public derivation (suffix step and wildcard).
            ("hardened step", keys(count, with_origin, "/0h/*")),
            ("hardened wildcard", keys(count, with_origin, "/0/*h")),
            // Ambiguous multipath.
            ("multipath", keys(count, with_origin, "/<0;1>/*")),
            // Private key in the last position (a multisig admits it in
            // any slot, so a single secret refuses the whole descriptor).
            ("private xprv", {
                let mut k = keys(count, with_origin, "/0/*");
                let last = count - 1;
                k[last] = match origin_of(last) {
                    Some(o) => format!("[{o}]{}/0/*", xpriv(60)),
                    None => format!("{}/0/*", xpriv(60)),
                };
                k
            }),
            ("private wif", {
                let mut k = keys(count, with_origin, "/0/*");
                let wif =
                    bitcoin::PrivateKey::new(xpriv(61).private_key, bitcoin::Network::Bitcoin)
                        .to_wif();
                k[0] = match origin_of(0) {
                    Some(o) => format!("[{o}]{wif}"),
                    None => wif,
                };
                k
            }),
            // Non-mainnet extended key.
            ("testnet xpub", {
                let mut k = keys(count, with_origin, "/0/*");
                let tpub = Xpub::from_priv(
                    secp,
                    &Xpriv::new_master(bitcoin::Network::Testnet, &[62; 32]).unwrap(),
                );
                k[0] = match origin_of(0) {
                    Some(o) => format!("[{o}]{tpub}/0/*"),
                    None => format!("{tpub}/0/*"),
                };
                k
            }),
        ];
        for (why, k) in refused {
            let text = shape(&k);
            assert_eq!(
                ScanDescriptor::parse(Branch::External, &text).unwrap_err(),
                ScanError::Descriptor,
                "{name} {why} origin={with_origin}: {text}"
            );
        }
    }
}

//! Positive, short-lived observations for outputs already owned on Bitcoin.
//! This is display evidence only: it cannot authorize any transaction.
use super::*;
use std::{collections::BTreeMap, time::Instant};

pub const NOTICE_MAX_AGE: Duration = Duration::from_secs(60);
const FORK_HEIGHT: u32 = 961_640;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownOutput {
    pub outpoint: OutPoint,
    pub output: bitcoin::TxOut,
    pub bitcoin_height: u32,
}

#[derive(Debug, Clone)]
pub struct KnownUnspent {
    outputs: Vec<KnownOutput>,
    generation: u64,
    live: watch::Receiver<u64>,
    observed_at: Instant,
}
impl KnownUnspent {
    #[cfg(test)]
    pub(crate) fn for_notice_test(
        outputs: Vec<KnownOutput>,
        generation: u64,
        live: watch::Receiver<u64>,
        observed_at: Instant,
    ) -> Self {
        Self {
            outputs,
            generation,
            live,
            observed_at,
        }
    }

    pub fn outputs(&self) -> &[KnownOutput] {
        &self.outputs
    }
    pub fn is_current(&self, expected: u64, now: Instant) -> bool {
        self.generation == expected
            && *self.live.borrow() == expected
            && self.live.has_changed().is_ok()
            && now.saturating_duration_since(self.observed_at) < NOTICE_MAX_AGE
    }
}

/// Obtain positive fork-unspent evidence for a supplied, owned pre-fork set.
/// Empty/absent results do not establish that a wallet has been swept. The
/// caller must match returned outputs to its current owned coins on receipt
/// and at display time, and revoke generation on account/provider/Cube changes.
pub async fn known_unspent(
    client: CoincubeClient,
    outputs: Vec<KnownOutput>,
    expected: u64,
    mut generation: watch::Receiver<u64>,
) -> Result<Option<KnownUnspent>, ScanError> {
    validate(&outputs)?;
    if generation.has_changed().is_err() || *generation.borrow() != expected {
        return Err(ScanError::Cancelled);
    }
    let source = http::HttpSource::new(client)?;
    // Conservative age: include the entire collection, not just its delivery.
    let observed_at = Instant::now();
    let cancelled = async {
        loop {
            if generation.changed().await.is_err() || *generation.borrow_and_update() != expected {
                break;
            }
        }
    };
    let found = tokio::select! { biased;
        _ = cancelled => return Err(ScanError::Cancelled),
        result = tokio::time::timeout(MAX_DURATION, collect_known(&source, &outputs)) => result.map_err(|_| ScanError::Deadline)??,
    };
    if generation.has_changed().is_err() || *generation.borrow() != expected {
        return Err(ScanError::Cancelled);
    }
    Ok((!found.is_empty()).then_some(KnownUnspent {
        outputs: found,
        generation: expected,
        live: generation,
        observed_at,
    }))
}

fn validate(outputs: &[KnownOutput]) -> Result<(), ScanError> {
    if outputs.is_empty() || outputs.len() > MAX_UTXOS {
        return Err(ScanError::InvalidLimits);
    }
    let mut seen = BTreeSet::new();
    let mut scripts = BTreeSet::new();
    for coin in outputs {
        if coin.outpoint.is_null()
            || !seen.insert(coin.outpoint)
            || coin.bitcoin_height == 0
            || coin.bitcoin_height >= FORK_HEIGHT
            || coin.output.value > bitcoin::Amount::MAX_MONEY
        {
            return Err(ScanError::Prevout);
        }
        scripts.insert(&coin.output.script_pubkey);
    }
    if scripts.len() > MAX_ADDRESSES as usize {
        return Err(ScanError::AddressLimit);
    }
    Ok(())
}

async fn collect_known(
    source: &impl Source,
    outputs: &[KnownOutput],
) -> Result<Vec<KnownOutput>, ScanError> {
    validate(outputs)?;
    let chain = ChainId::BitcoinBlake2b;
    let tip = source.tip(chain).await?;
    // `anchor` also reports the observed fork height (Split); only the
    // anchored tip hash matters for this known-output check.
    if source.anchor().await?.0 != tip {
        return Err(ScanError::Changed(ChangedReason::AnchorMismatch));
    }
    let mut addresses: BTreeMap<String, Vec<&KnownOutput>> = BTreeMap::new();
    for coin in outputs {
        let address =
            bitcoin::Address::from_script(&coin.output.script_pubkey, bitcoin::Network::Bitcoin)
                .map_err(|_| ScanError::Descriptor)?;
        addresses.entry(address.to_string()).or_default().push(coin);
    }
    let mut found = Vec::new();
    let mut snapshots = Vec::new();
    for (address, expected) in addresses {
        let before = collect_known_address(source, chain, &address, &expected, &mut found).await?;
        snapshots.push((address, before));
    }
    // Recheck all addresses after raw transactions have been fetched, so a
    // spend during a later address lookup also invalidates earlier evidence.
    for (address, before) in snapshots {
        if !same_utxos(&before, &source.utxos(chain, &address).await?)? {
            return Err(ScanError::Changed(ChangedReason::AddressState));
        }
    }
    if tip != source.tip(chain).await? {
        return Err(ScanError::Changed(ChangedReason::TipMoved));
    }
    if tip != source.anchor().await?.0 {
        return Err(ScanError::Changed(ChangedReason::AnchorMismatch));
    }
    Ok(found)
}

async fn collect_known_address(
    source: &impl Source,
    chain: ChainId,
    address: &str,
    expected: &[&KnownOutput],
    found: &mut Vec<KnownOutput>,
) -> Result<Vec<Utxo>, ScanError> {
    let before = source.utxos(chain, address).await?;
    if before.len() > MAX_UTXOS {
        return Err(ScanError::BodyLimit);
    }
    let mut seen = BTreeSet::new();
    for coin in &before {
        let outpoint = OutPoint {
            txid: coin.txid,
            vout: coin.vout,
        };
        if !seen.insert(outpoint) {
            return Err(ScanError::Malformed);
        }
        let Some(known) = expected.iter().find(|c| c.outpoint == outpoint) else {
            continue;
        };
        if !coin.status.confirmed {
            continue;
        }
        if coin.status.block_height.is_none() || coin.status.block_hash.is_none() {
            return Err(ScanError::Malformed);
        }
        let tx = source.transaction(chain, coin.txid).await?;
        if tx.compute_txid() != coin.txid
            || tx.output.get(coin.vout as usize) != Some(&known.output)
            || coin.value != known.output.value.to_sat()
        {
            return Err(ScanError::Prevout);
        }
        found.push((*known).clone());
    }

    Ok(before)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::hashes::Hash;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Fixture {
        tx: Transaction,
        coin: Utxo,
        changed: bool,
        unavailable: bool,
        calls: AtomicUsize,
    }
    impl Fixture {
        fn new() -> Self {
            let tx = Transaction {
                version: bitcoin::transaction::Version::TWO,
                lock_time: bitcoin::absolute::LockTime::ZERO,
                input: vec![bitcoin::TxIn::default()],
                output: vec![bitcoin::TxOut {
                    value: bitcoin::Amount::from_sat(1000),
                    script_pubkey: ScriptBuf::new_p2wpkh(&bitcoin::WPubkeyHash::from_byte_array(
                        [3; 20],
                    )),
                }],
            };
            let coin = Utxo {
                txid: tx.compute_txid(),
                vout: 0,
                value: 1000,
                status: Status {
                    confirmed: true,
                    block_height: Some(900_000),
                    block_hash: Some(BlockHash::from_byte_array([2; 32])),
                },
            };
            Self {
                tx,
                coin,
                changed: false,
                unavailable: false,
                calls: AtomicUsize::new(0),
            }
        }
        fn known(&self) -> KnownOutput {
            KnownOutput {
                outpoint: OutPoint {
                    txid: self.coin.txid,
                    vout: 0,
                },
                output: self.tx.output[0].clone(),
                bitcoin_height: 900_000,
            }
        }
    }
    #[async_trait]
    impl Source for Fixture {
        async fn tip(&self, _: ChainId) -> Result<BlockHash, ScanError> {
            Ok(BlockHash::from_byte_array([1; 32]))
        }
        async fn tip_height(&self, _: ChainId, _: BlockHash) -> Result<u32, ScanError> {
            unreachable!("known-output collection reads no tip height")
        }
        async fn anchor(&self) -> Result<(BlockHash, Option<u64>), ScanError> {
            Ok((self.tip(ChainId::BitcoinBlake2b).await?, None))
        }
        async fn stats(&self, _: ChainId, _: &str) -> Result<Stats, ScanError> {
            panic!("known-output lookup must not enumerate history")
        }
        async fn utxos(&self, _: ChainId, _: &str) -> Result<Vec<Utxo>, ScanError> {
            if self.unavailable {
                return Err(ScanError::Unavailable);
            }
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(if self.changed && n > 0 {
                vec![]
            } else {
                vec![self.coin.clone()]
            })
        }
        async fn transaction(&self, _: ChainId, _: Txid) -> Result<Transaction, ScanError> {
            Ok(self.tx.clone())
        }
    }

    struct ReorderedCoins {
        tx: Transaction,
        coins: Vec<Utxo>,
        reads: AtomicUsize,
    }
    #[async_trait]
    impl Source for ReorderedCoins {
        async fn tip(&self, _: ChainId) -> Result<BlockHash, ScanError> {
            Ok(BlockHash::from_byte_array([1; 32]))
        }
        async fn tip_height(&self, _: ChainId, _: BlockHash) -> Result<u32, ScanError> {
            unreachable!()
        }
        async fn anchor(&self) -> Result<(BlockHash, Option<u64>), ScanError> {
            Ok((BlockHash::from_byte_array([1; 32]), None))
        }
        async fn stats(&self, _: ChainId, _: &str) -> Result<Stats, ScanError> {
            unreachable!()
        }
        async fn utxos(&self, _: ChainId, _: &str) -> Result<Vec<Utxo>, ScanError> {
            let mut coins = self.coins.clone();
            if self.reads.fetch_add(1, Ordering::SeqCst) == 1 {
                coins.reverse();
            }
            Ok(coins)
        }
        async fn transaction(&self, _: ChainId, _: Txid) -> Result<Transaction, ScanError> {
            Ok(self.tx.clone())
        }
    }

    #[tokio::test]
    async fn known_unspent_accepts_reordered_identical_outputs() {
        let mut tx = Fixture::new().tx;
        tx.output.push(bitcoin::TxOut {
            value: bitcoin::Amount::from_sat(2000),
            script_pubkey: tx.output[0].script_pubkey.clone(),
        });
        let txid = tx.compute_txid();
        let block_hash = BlockHash::from_byte_array([2; 32]);
        let coins: Vec<_> = tx
            .output
            .iter()
            .enumerate()
            .map(|(vout, output)| Utxo {
                txid,
                vout: vout as u32,
                value: output.value.to_sat(),
                status: Status {
                    confirmed: true,
                    block_height: Some(900_000),
                    block_hash: Some(block_hash),
                },
            })
            .collect();
        let known: Vec<_> = tx
            .output
            .iter()
            .enumerate()
            .map(|(vout, output)| KnownOutput {
                outpoint: OutPoint {
                    txid,
                    vout: vout as u32,
                },
                output: output.clone(),
                bitcoin_height: 900_000,
            })
            .collect();
        let source = ReorderedCoins {
            tx,
            coins,
            reads: AtomicUsize::new(0),
        };
        assert_eq!(collect_known(&source, &known).await, Ok(known));
    }
    #[tokio::test]
    async fn positive_requires_exact_prevout_and_stable_unspent_observation() {
        let mut source = Fixture::new();
        let known = source.known();
        assert_eq!(
            collect_known(&source, std::slice::from_ref(&known))
                .await
                .unwrap(),
            vec![known.clone()]
        );
        source.coin.value += 1;
        assert_eq!(
            collect_known(&source, std::slice::from_ref(&known)).await,
            Err(ScanError::Prevout)
        );
        source.coin.value -= 1;
        source.changed = true;
        source.calls.store(0, Ordering::SeqCst);
        assert_eq!(
            collect_known(&source, std::slice::from_ref(&known)).await,
            Err(ScanError::Changed(ChangedReason::AddressState))
        );
        source.unavailable = true;
        assert_eq!(
            collect_known(&source, &[known]).await,
            Err(ScanError::Unavailable)
        );
    }
    #[tokio::test]
    async fn unconfirmed_is_not_positive_and_substituted_raw_transaction_refuses() {
        let mut source = Fixture::new();
        let known = source.known();
        source.coin.status.confirmed = false;
        assert!(collect_known(&source, std::slice::from_ref(&known))
            .await
            .unwrap()
            .is_empty());
        source.coin.status.confirmed = true;
        source.tx.output[0].value += bitcoin::Amount::from_sat(1);
        assert_eq!(
            collect_known(&source, &[known]).await,
            Err(ScanError::Prevout)
        );
    }
    #[tokio::test]
    async fn revoked_generation_refuses_before_network() {
        let (tx, rx) = watch::channel(2);
        let result = known_unspent(
            CoincubeClient::for_test("http://127.0.0.1:1"),
            vec![Fixture::new().known()],
            1,
            rx,
        )
        .await;
        assert!(matches!(result, Err(ScanError::Cancelled)));
        drop(tx);
    }
    #[test]
    fn evidence_expires_and_cannot_survive_revocation_or_session_drop() {
        let (tx, rx) = watch::channel(4);
        let now = Instant::now();
        let proof = KnownUnspent {
            outputs: vec![Fixture::new().known()],
            generation: 4,
            live: rx,
            observed_at: now,
        };
        assert!(proof.is_current(4, now));
        assert!(!proof.is_current(5, now));
        assert!(!proof.is_current(4, now + NOTICE_MAX_AGE));
        tx.send_replace(5);
        assert!(!proof.is_current(4, now));
        drop(tx);
        assert!(!proof.is_current(4, now));
    }
    #[test]
    fn local_inputs_must_be_unique_confirmed_and_pre_fork() {
        let mut known = Fixture::new().known();
        assert_eq!(
            validate(&[known.clone(), known.clone()]),
            Err(ScanError::Prevout)
        );
        known.bitcoin_height = FORK_HEIGHT;
        assert_eq!(validate(&[known.clone()]), Err(ScanError::Prevout));
        known.bitcoin_height = 0;
        assert_eq!(validate(&[known]), Err(ScanError::Prevout));
    }
    #[tokio::test]
    async fn http_known_output_positive_absent_invalid_stale_and_unavailable() {
        for case in 0..8 {
            check_http_known_case(case).await;
        }
    }
    async fn check_http_known_case(case: i32) {
        use httpmock::prelude::*;
        use serde_json::json;
        use std::time::{SystemTime, UNIX_EPOCH};

        let server = MockServer::start();
        let f = Fixture::new();
        let known = f.known();
        let address =
            bitcoin::Address::from_script(&known.output.script_pubkey, bitcoin::Network::Bitcoin)
                .unwrap();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let anchor_body = json!({"success":true,"data":{"network":"bitcoin-blake2b","state":"available","anchor":{
            "tip_hash":"11".repeat(32),"tip_height":973029,"tip_median_time_past":1800000000,
            "observed_at": if case == 4 { now - 120 } else { now },
            "observation":{"tip_height":973029,"fork":{"height":961640,"active":true},
                "rdts":{"state":"flagday","flagday":{"height":961640,"expiry_time":1800010000_i64,"active":false}}}
        }}});
        let anchor = server.mock(|when, then| {
            when.method(GET)
                .path("/api/v1/connect/networks/bitcoin-blake2b/anchor")
                .header("authorization", "Bearer synthetic-notice-token");
            then.status(200).json_body(anchor_body);
        });
        let tip = server.mock(|when, then| {
            when.method(GET)
                .path("/api/v1/esplora/bitcoin-blake2b/mainnet/blocks/tip/hash");
            then.status(200)
                .header("X-Coincube-Observation", "fresh")
                .header("X-Cache", "BYPASS")
                .header("Cache-Control", "no-store")
                .body("11".repeat(32));
        });
        let body = match case {
                1 => "[]".to_string(),
                3 => "not-json".to_string(),
                _ => json!([{"txid":f.coin.txid.to_string(),"vout":0,"value":if case == 2 { 1001 } else { 1000 },
                    "status":{"confirmed":true,"block_height":900000,"block_hash":"22".repeat(32)}}]).to_string(),
            };
        let utxos = server.mock(|when, then| {
            when.method(GET)
                .path(format!(
                    "/api/v1/esplora/bitcoin-blake2b/mainnet/address/{}/utxo",
                    address
                ))
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
            let then = then.status(if case == 6 { 503 } else { 200 });
            let then = if case == 5 {
                then
            } else {
                then.header("X-Coincube-Observation", "fresh")
                    .header("X-Cache", "BYPASS")
                    .header("Cache-Control", "no-store")
            };
            let then = if case == 7 {
                then.delay(Duration::from_secs(10))
            } else {
                then
            };
            then.body(body);
        });
        let raw = server.mock(|when, then| {
            when.method(GET)
                .path(format!(
                    "/api/v1/esplora/bitcoin-blake2b/mainnet/tx/{}/hex",
                    f.coin.txid
                ))
                .matches(|r| {
                    r.headers.as_ref().is_none_or(|hs| {
                        hs.iter()
                            .all(|(n, _)| !n.eq_ignore_ascii_case("authorization"))
                    })
                });
            then.status(200)
                .body(hex::encode(bitcoin::consensus::serialize(&f.tx)));
        });
        let mut client = CoincubeClient::for_test(server.base_url());
        client.set_token("synthetic-notice-token");
        let (live, rx) = watch::channel(7);
        let result = run_http_known_case(case, client, &known, &live, rx, &utxos).await;
        match case {
            0 => {
                let proof = result.unwrap().unwrap();
                assert_eq!(proof.outputs(), &[known]);
                assert!(proof.is_current(7, Instant::now()));
                anchor.assert_hits(2);
                tip.assert_hits(2);
                utxos.assert_hits(2);
                raw.assert_hits(1);
            }
            1 => {
                assert!(result.unwrap().is_none());
                raw.assert_hits(0);
            }
            2 => assert!(matches!(result, Err(ScanError::Prevout))),
            3 => assert!(matches!(result, Err(ScanError::Malformed))),
            4 | 5 => assert!(matches!(result, Err(ScanError::Freshness))),
            6 => assert!(matches!(result, Err(ScanError::Http(503)))),
            7 => assert!(matches!(result, Err(ScanError::Cancelled))),
            _ => unreachable!(),
        }
    }

    async fn run_http_known_case(
        case: i32,
        client: CoincubeClient,
        known: &KnownOutput,
        live: &watch::Sender<u64>,
        rx: watch::Receiver<u64>,
        utxos: &httpmock::Mock<'_>,
    ) -> Result<Option<KnownUnspent>, ScanError> {
        if case == 7 {
            let task = tokio::spawn(known_unspent(client, vec![known.clone()], 7, rx));
            for _ in 0..100 {
                if utxos.hits() > 0 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            assert_eq!(utxos.hits(), 1);
            live.send_replace(8);
            task.await.unwrap()
        } else {
            known_unspent(client, vec![known.clone()], 7, rx).await
        }
    }
}

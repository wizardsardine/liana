//! The Esplora backend against a scripted server: a block height out of our
//! range fails the poll before any of it is applied, rather than panicking under
//! the backend lock (#616, #621).

use super::*;
use crate::bitcoin::BitcoinInterface;
use bitcoin::{
    absolute,
    hashes::{sha256, Hash},
    transaction::Version,
};
use std::{
    io::{self, BufRead, BufReader, Write},
    net::TcpListener,
    str::FromStr,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

const DESCRIPTOR: &str = concat!(
    "wsh(andor(pk([aabbccdd]xpub68JJTXc1MWK8KLW4HGLXZBJknja7kDUJuFHnM424LbziEXsfkh1WQCiEjjHw4z",
    "LqSUm4rvhgyGkkuRowE9tCJSgt3TQB5J3SKAbZ2SdcKST/<0;1>/*),older(10000),pk([aabbccdd]xpub68JJT",
    "Xc1MWK8PEQozKsRatrUHXKFNkD1Cb1BuQU9Xr5moCv87anqGyXLyUd4KpnDyZgo3gz4aN1r3NiaoweFW8Uut",
    "BsBbgKHzaD5HkTkifK/<0;1>/*)))#3xh8xmhn"
);

const OUT_OF_RANGE: &str = "out-of-range block height";

fn serve_connection(stream: std::net::TcpStream, answer: impl Fn(&str) -> Option<String>) {
    let mut writer = stream.try_clone().unwrap();
    let mut reader = BufReader::new(stream);
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return;
    }
    // Read the rest of the head: a GET has no body.
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => return,
            Ok(_) if line == "\r\n" => break,
            Ok(_) => {}
        }
    }
    let path = request_line.split_whitespace().nth(1).unwrap_or("");
    let (status, body) = match answer(path) {
        Some(body) => ("200 OK", body),
        None => ("404 Not Found", "not found".to_string()),
    };
    let _ = write!(
        writer,
        "HTTP/1.1 {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        status,
        body.len(),
        body
    );
    let _ = writer.flush();
}

/// An HTTP server answering every `GET path` with `answer(path)`: a status and a
/// body, or `None` for a 404. One request per connection.
fn scripted_server(
    listener: TcpListener,
    answer: impl Fn(&str) -> Option<String> + Send + Sync + 'static,
    stop: mpsc::Receiver<()>,
) -> thread::JoinHandle<()> {
    // Nonblocking on this handle, so the stop signal is seen between
    // connections (a blocking accept hung the Windows CI job in #591).
    listener.set_nonblocking(true).unwrap();
    let answer = Arc::new(answer);
    thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(300);
        loop {
            let stream = match listener.accept() {
                Ok((stream, _)) => stream,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    if stop.try_recv().is_ok() {
                        return;
                    }
                    assert!(
                        Instant::now() < deadline,
                        "scripted server was never stopped"
                    );
                    thread::sleep(Duration::from_millis(1));
                    continue;
                }
                other => panic!("Esplora test accept failed: {:?}", other),
            };
            // Accepted sockets can inherit the listener's nonblocking mode.
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(30)))
                .unwrap();
            let answer = answer.clone();
            // The client sends requests in parallel during a scan.
            thread::spawn(move || {
                serve_connection(stream, |path| answer(path));
            });
        }
    })
}

fn genesis_hash() -> BlockHash {
    bitcoin::blockdata::constants::genesis_block(bitcoin::Network::Regtest).block_hash()
}

/// The hash of the scripted chain's block at `height`.
fn hash_at(height: u64) -> BlockHash {
    if height == 0 {
        return genesis_hash();
    }
    BlockHash::from_byte_array(sha256::Hash::hash(&height.to_le_bytes()).to_byte_array())
}

/// A server whose chain tip is at `tip` (read at each request, so a test can move
/// it). Script histories are empty, except those `history` answers.
fn tip_at(
    tip: Arc<AtomicU64>,
    history: impl Fn(&str) -> Option<String> + Send + Sync + 'static,
) -> impl Fn(&str) -> Option<String> + Send + Sync + 'static {
    move |path| {
        let tip = tip.load(Ordering::SeqCst);
        let merkle = "33".repeat(32);
        if path == "/blocks/tip/height" {
            return Some(tip.to_string());
        }
        if path == "/blocks/tip/hash" {
            return Some(hash_at(tip).to_string());
        }
        if path == "/blocks" {
            return Some(format!(
                r#"[{{"id":"{}","height":{},"timestamp":1700000000,"previousblockhash":"{}","merkle_root":"{}"}}]"#,
                hash_at(tip),
                tip,
                hash_at(tip.saturating_sub(1)),
                merkle
            ));
        }
        if let Some(height) = path.strip_prefix("/block-height/") {
            return Some(hash_at(height.parse().ok()?).to_string());
        }
        if let Some(rest) = path.strip_prefix("/block/") {
            let hash = rest.strip_suffix("/status")?;
            let height = (0..=tip).rev().find(|h| hash_at(*h).to_string() == hash)?;
            return Some(format!(
                r#"{{"in_best_chain":true,"height":{},"next_best":null}}"#,
                height
            ));
        }
        if let Some(rest) = path.strip_prefix("/scripthash/") {
            let scripthash = rest.split('/').next()?;
            return Some(history(scripthash).unwrap_or_else(|| "[]".to_string()));
        }
        None
    }
}

/// Run `call` against an Esplora backend whose wallet chain is at genesis,
/// connected to a server answering with `answer`.
fn against_server<T>(
    answer: impl Fn(&str) -> Option<String> + Send + Sync + 'static,
    call: impl FnOnce(Esplora) -> T,
) -> T {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = format!("http://{}", listener.local_addr().unwrap());
    let (stop, stopped) = mpsc::channel();
    let server = scripted_server(listener, answer, stopped);
    let client = client::Client::new(
        &crate::config::EsploraConfig {
            addr,
            token: None,
            fallback_addr: None,
            fallback_token: None,
            secondary_fallback_addr: None,
            secondary_fallback_token: None,
        },
        Arc::new(AtomicBool::new(false)),
    )
    .unwrap();
    let wallet = BdkWallet::new(
        &DESCRIPTOR.parse().unwrap(),
        genesis_hash(),
        None,
        &[],
        &[],
        0.into(),
        0.into(),
    );
    let backend = Esplora::new(client, wallet, false).unwrap();
    let result = call(backend);
    stop.send(()).unwrap();
    server.join().unwrap();
    result
}

/// Poll `shared`: it fails on an out-of-range height, and nothing of the update
/// was applied, so the poller's next reads of the tip see `expected_tip`
/// instead of panicking under the lock. Returns the warnings it logged.
fn assert_poll_refused(
    shared: &mut Arc<Mutex<dyn BitcoinInterface>>,
    expected_tip: u64,
) -> Vec<String> {
    assert_poll_refused_with(shared, expected_tip, OUT_OF_RANGE)
}

/// [`assert_poll_refused`], for a refusal whose error contains `error`.
fn assert_poll_refused_with(
    shared: &mut Arc<Mutex<dyn BitcoinInterface>>,
    expected_tip: u64,
    error: &str,
) -> Vec<String> {
    let (sync, logs) = crate::testutils::capture_logs(|| shared.sync_wallet(0.into(), 0.into()));
    assert!(matches!(&sync, Err(e) if e.contains(error)), "{:?}", sync);
    let tip = shared.chain_tip();
    assert_eq!(
        (tip.height as u64, tip.hash),
        (expected_tip, hash_at(expected_tip))
    );
    assert_eq!(shared.sync_progress().blocks, expected_tip);
    assert!(!shared.is_poisoned());
    let warnings: Vec<_> = logs
        .into_iter()
        .filter(|(level, _)| *level == log::Level::Warn)
        .map(|(_, message)| message)
        .collect();
    // Visible at the default log level, with the reason and no server address.
    assert_eq!(warnings.len(), 1, "{:?}", warnings);
    assert!(
        warnings[0].contains("out of range") && !warnings[0].contains("127.0.0.1"),
        "{:?}",
        warnings
    );
    warnings
}

#[test]
fn esplora_out_of_range_tip_height_fails_the_poll_and_leaves_the_wallet_tip_alone() {
    for out_of_range in [i32::MAX as u64 + 1, u32::MAX as u64] {
        let tip = Arc::new(AtomicU64::new(out_of_range));
        against_server(tip_at(tip.clone(), |_| None), |backend| {
            let mut shared: Arc<Mutex<dyn BitcoinInterface>> = Arc::new(Mutex::new(backend));

            // The wallet's chain is at genesis: the poll is a full scan, and the
            // next one is still a full scan.
            let warnings = assert_poll_refused(&mut shared, 0);
            assert!(warnings[0].contains("Refused the Esplora chain update"));
            assert_eq!(shared.rescan_progress(), Some(0.0));
            assert_poll_refused(&mut shared, 0);
            assert_eq!(shared.rescan_progress(), Some(0.0));

            // A sane tip: the wallet's chain moves there, and the next poll is an
            // incremental sync.
            tip.store(20, Ordering::SeqCst);
            let sync = shared.sync_wallet(0.into(), 0.into());
            assert!(matches!(sync, Ok(None)), "{:?}", sync);
            assert_eq!(shared.chain_tip().height, 20);
            assert_eq!(shared.rescan_progress(), None);

            // The incremental poll first reads the tip, and refuses it.
            tip.store(out_of_range, Ordering::SeqCst);
            let warnings = assert_poll_refused(&mut shared, 20);
            assert!(warnings[0].contains("Refused the Esplora chain tip"));
            assert_eq!(shared.rescan_progress(), None);

            // An eager sync skips that read: its update is refused instead.
            shared.request_eager_sync();
            let warnings = assert_poll_refused(&mut shared, 20);
            assert!(warnings[0].contains("Refused the Esplora chain update"));

            // A rescan whose first poll is refused stays a rescan: had the
            // refusal cleared it, the poller would take it for complete.
            let desc = DESCRIPTOR.parse().unwrap();
            shared.start_rescan(&desc, 0).unwrap();
            assert_poll_refused(&mut shared, 20);
            assert_eq!(shared.rescan_progress(), Some(0.0));

            tip.store(20, Ordering::SeqCst);
            let sync = shared.sync_wallet(0.into(), 0.into());
            assert!(sync.is_ok(), "{:?}", sync);
            assert_eq!(shared.rescan_progress(), None);
            assert!(!shared.is_poisoned());
        });
    }
}

/// A transaction paying to the wallet, which the server says confirmed at
/// `height` with block time `time`, one of them out of our range: the poll that
/// returns it is refused with `error` and warns `warning`, and nothing of it is
/// applied, so reading the transaction neither panics nor poisons the lock.
fn assert_confirmation_refused(height: u64, time: u64, error: &str, warning: &str) {
    let tip = Arc::new(AtomicU64::new(20));
    let spk: Arc<Mutex<Option<bitcoin::ScriptBuf>>> = Arc::new(Mutex::new(None));
    let tx_of = |spk: bitcoin::ScriptBuf| bitcoin::Transaction {
        version: Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: vec![bitcoin::TxIn {
            previous_output: OutPoint::new(
                bitcoin::Txid::from_str(
                    "4a5e1e4baab89f3a32518a88c31bc87f618f76673e2cc77ab2127b7afdeda33b",
                )
                .unwrap(),
                0,
            ),
            script_sig: bitcoin::ScriptBuf::new(),
            sequence: bitcoin::Sequence::ENABLE_RBF_NO_LOCKTIME,
            witness: bitcoin::Witness::default(),
        }],
        output: vec![bitcoin::TxOut {
            value: bitcoin::Amount::from_sat(100_000),
            script_pubkey: spk,
        }],
    };
    let history = {
        let spk = spk.clone();
        move |scripthash: &str| {
            let spk = spk.lock().unwrap().clone()?;
            if format!("{:x}", sha256::Hash::hash(spk.as_bytes())) != scripthash {
                return None;
            }
            let tx = tx_of(spk.clone());
            Some(format!(
                r#"[{{"txid":"{}","version":2,"locktime":0,"vin":[{{"txid":"{}","vout":0,"prevout":null,"scriptsig":"","witness":[],"sequence":{},"is_coinbase":false}}],"vout":[{{"value":100000,"scriptpubkey":"{}"}}],"status":{{"confirmed":true,"block_height":{},"block_hash":"{}","block_time":{}}},"fee":1000}}]"#,
                tx.compute_txid(),
                tx.input[0].previous_output.txid,
                tx.input[0].sequence.0,
                spk.to_hex_string(),
                height,
                hash_at(height),
                time,
            ))
        }
    };
    against_server(tip_at(tip, history), |backend| {
        // The wallet's first receive script, which every scan asks for.
        let wallet_spk = backend
            .bdk_wallet
            .index()
            .inner()
            .all_spks()
            .values()
            .next()
            .cloned()
            .unwrap();
        let txid = tx_of(wallet_spk.clone()).compute_txid();
        *spk.lock().unwrap() = Some(wallet_spk);
        let mut shared: Arc<Mutex<dyn BitcoinInterface>> = Arc::new(Mutex::new(backend));

        let warnings = assert_poll_refused_with(&mut shared, 0, error);
        assert!(warnings[0].contains(warning), "{:?}", warnings);
        assert_eq!(shared.rescan_progress(), Some(0.0));
        // Nothing of it was applied: the transaction is unknown, not a panic.
        assert!(shared.wallet_transaction(&txid).is_none());
        assert!(!shared.is_poisoned());
    });
}

#[test]
fn esplora_out_of_range_confirmation_height_fails_the_poll_and_is_never_read() {
    // Far above the sane tip.
    assert_confirmation_refused(
        u32::MAX as u64,
        1_700_000_000,
        OUT_OF_RANGE,
        "graph update: the server reported a confirmation at block height",
    );
}

#[test]
fn esplora_out_of_range_confirmation_time_fails_the_poll_and_is_never_read() {
    // At a sane height with its real hash: only the block time is out of range,
    // and BDK copies it into the anchor unchecked.
    assert_confirmation_refused(
        5,
        1 << 40,
        "out-of-range block time",
        "graph update: the server reported a confirmation with block time",
    );
}

/// A full scan run by the real backend reports into the history record: it shows as
/// running, with addresses counted, while the server is answering its lookups; and
/// once it returns it is held for the poll that commits it, not shown as complete.
#[test]
fn esplora_full_scan_reports_its_progress_into_the_history_record() {
    let cache = Arc::new(crate::bitcoin::HistorySyncCache::default());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let history = {
        let cache = cache.clone();
        let seen = seen.clone();
        move |_scripthash: &str| {
            seen.lock().unwrap().push(cache.snapshot());
            None
        }
    };
    let sync = against_server(
        tip_at(Arc::new(AtomicU64::new(10)), history),
        |mut backend| {
            BitcoinInterface::set_history_sync_cache(&mut backend, cache.clone());
            backend.sync_wallet(0.into(), 0.into())
        },
    );
    assert!(sync.is_ok(), "{:?}", sync);

    let seen = seen.lock().unwrap();
    assert!(!seen.is_empty(), "the scan looked up no address");
    assert!(seen.iter().all(|s| s.full_scan_in_progress));
    // Two keychains with index 0 revealed, as on a fresh restore: one address
    // plus a stop gap's worth on each.
    assert!(seen.iter().all(|s| s.addresses_expected == 402));
    let last = seen.iter().map(|s| s.addresses_checked).max().unwrap();
    assert!(last >= 200, "only {} addresses counted", last);

    let after = cache.snapshot();
    assert!(!after.full_scan_in_progress);
    assert_eq!(after.full_scan_completed_at, None);
    cache.poll_succeeded(42);
    assert_eq!(cache.snapshot().full_scan_completed_at, Some(42));
}

#[test]
fn public_readiness_checks_genesis_and_tip_without_constructing_a_wallet() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let config = crate::config::EsploraConfig {
        addr: format!("http://{}", listener.local_addr().unwrap()),
        token: None,
        fallback_addr: None,
        fallback_token: None,
        secondary_fallback_addr: None,
        secondary_fallback_token: None,
    };
    let (stop, stopped) = mpsc::channel();
    let server = scripted_server(
        listener,
        tip_at(Arc::new(AtomicU64::new(2)), |_| None),
        stopped,
    );
    let ready = Esplora::public_backend_ready(&config, &genesis_hash());
    let wrong_chain = Esplora::public_backend_ready(&config, &hash_at(1));
    stop.send(()).unwrap();
    server.join().unwrap();
    assert!(matches!(ready, Ok(true)));
    assert!(matches!(wrong_chain, Ok(false)));
}

/// The poller holds the backend's mutex for a whole wallet sync. A broadcast
/// must reach the server during it instead of queueing behind it.
#[test]
fn esplora_broadcast_reaches_the_server_while_the_backend_is_locked_for_a_sync() {
    let tx = bitcoin::Transaction {
        version: Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: vec![bitcoin::TxIn::default()],
        output: vec![],
    };
    let txid = tx.compute_txid().to_string();
    let posted = Arc::new(AtomicBool::new(false));
    let chain = tip_at(Arc::new(AtomicU64::new(0)), |_| None);
    let answer = {
        let posted = posted.clone();
        move |path: &str| {
            if path == "/tx" {
                posted.store(true, Ordering::SeqCst);
                return Some(txid.clone());
            }
            chain(path)
        }
    };
    let sent = against_server(answer, |backend| {
        let shared: Arc<Mutex<dyn BitcoinInterface>> = Arc::new(Mutex::new(backend));
        let broadcast = shared
            .detached_broadcaster()
            .expect("the Esplora backend broadcasts without its lock");
        let _syncing = shared.lock().unwrap();
        let (done, finished) = mpsc::channel();
        thread::spawn(move || done.send(broadcast(&tx)).unwrap());
        finished.recv_timeout(Duration::from_secs(10))
    });
    assert_eq!(
        sent.expect("the broadcast must not wait for the backend lock"),
        Ok(())
    );
    assert!(posted.load(Ordering::SeqCst));
}

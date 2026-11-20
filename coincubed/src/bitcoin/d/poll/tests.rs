use super::*;
use crate::database::DatabaseInterface;
use bitcoin::hashes::Hash;
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpListener},
    time::Instant,
};

fn backend(addr: SocketAddr) -> BitcoinD {
    let config = config::BitcoindConfig {
        addr,
        rpc_auth: config::BitcoindRpcAuth::UserPass("test".into(), "test".into()),
    };
    let client = |kind| RwLock::new(BitcoinD::build_client(&config, "poll-test", kind).unwrap());
    let mut bit = BitcoinD {
        local_fork_chain: None,
        poll_node_client: client(ClientKind::PollNode),
        poll_wallet_client: client(ClientKind::PollWallet),
        poll_abort: Default::default(),
        node_client: client(ClientKind::Node),
        watchonly_client: client(ClientKind::Watchonly),
        sendonly_client: client(ClientKind::Sendonly),
        sendonly_node_client: client(ClientKind::SendonlyNode),
        watchonly_wallet_path: "poll-test".into(),
        retries: BITCOIND_RETRY_LIMIT,
        config,
        record_replay: Default::default(),
    };
    bit.record_replay.acknowledge(1);
    bit
}

#[test]
fn replay_tickets_do_not_lose_a_request_arriving_during_an_older_poll() {
    use crate::bitcoin::BitcoinInterface;
    let mut bit = backend("127.0.0.1:1".parse().unwrap());
    let older = bit.request_wallet_record_replay().unwrap();
    let newer = bit.request_wallet_record_replay().unwrap();
    bit.acknowledge_wallet_record_replay(older);
    assert_eq!(bit.wallet_record_replay_pending(), Some(newer));
    bit.acknowledge_wallet_record_replay(newer);
    assert_eq!(bit.wallet_record_replay_pending(), None);
}

#[test]
fn a_full_poll_queue_leaves_replay_pending_without_blocking_the_controller() {
    use crate::bitcoin::BitcoinInterface;
    let mut control = command_control("127.0.0.1:1".parse().unwrap());
    let (sender, _receiver) = std::sync::mpsc::sync_channel(1);
    sender
        .try_send(crate::poller::PollerMessage::PollNowNoAck)
        .unwrap();
    control.poller_sender = sender;
    let started = Instant::now();
    assert!(control.replay_wallet_records().is_err());
    assert!(started.elapsed() < Duration::from_secs(1));
    assert!(control.bitcoin.wallet_record_replay_pending().is_some());
}

#[test]
fn recovered_core_spent_history_replays_into_the_daemon_at_an_unchanged_database_tip() {
    use crate::bitcoin::BitcoinInterface;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let mut control = command_control(listener.local_addr().unwrap());
    let root = std::env::temp_dir().join(format!(
        "record-replay-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let sqlite = crate::database::sqlite::SqliteDb::new(
        root.join("wallet.sqlite3"),
        Some(crate::database::sqlite::FreshDbOptions::new(
            control.config.bitcoin_config.chain,
            control.config.main_descriptor.as_vault().unwrap().clone(),
        )),
        &control.secp,
    )
    .unwrap();
    control.db = std::sync::Arc::new(std::sync::Mutex::new(sqlite));
    let tip = crate::bitcoin::BlockChainTip {
        height: 100,
        hash: bitcoin::BlockHash::from_byte_array([1; 32]),
    };
    control.db.connection().update_tip(&tip);
    let desc = control.config.main_descriptor.receive_descriptor();
    let address = desc
        .derive(500)
        .unwrap()
        .address(bitcoin::Network::Bitcoin)
        .unwrap();
    let mut funding =
        bitcoin::blockdata::constants::genesis_block(bitcoin::Network::Bitcoin).txdata[0].clone();
    funding.input[0].previous_output =
        bitcoin::OutPoint::new(bitcoin::Txid::from_byte_array([2; 32]), 0);
    funding.output[0].script_pubkey = address.script_pubkey();
    funding.output[0].value = bitcoin::Amount::from_sat(100_000_000);
    let txid = funding.compute_txid();
    let mut spender = funding.clone();
    spender.input[0].previous_output = bitcoin::OutPoint::new(txid, 0);
    spender.output[0].script_pubkey = bitcoin::ScriptBuf::from_bytes(vec![0x6a]);
    spender.output[0].value = bitcoin::Amount::from_sat(99_990_000);
    let spendid = spender.compute_txid();
    let spent_transaction = serde_json::json!({"hex":bitcoin::consensus::encode::serialize_hex(&spender),"confirmations":90,"blockhash":bitcoin::BlockHash::from_byte_array([4;32]),"blockheight":11,"blocktime":1700000600,"generated":false});
    let transaction = serde_json::json!({"hex":bitcoin::consensus::encode::serialize_hex(&funding),"confirmations":91,"blockhash":bitcoin::BlockHash::from_byte_array([3;32]),"blockheight":10,"blocktime":1700000000,"generated":false});
    let chain = serde_json::json!({"bestblockhash":tip.hash,"blocks":tip.height});
    let mut script = vec![
        ("getwalletinfo", serde_json::json!({"scanning":false})),
        ("getblockchaininfo", chain.clone()),
        (
            "getblockhash",
            serde_json::json!(bitcoin::blockdata::constants::genesis_block(
                bitcoin::Network::Bitcoin
            )
            .block_hash()),
        ),
        (
            "listsinceblock",
            serde_json::json!({"transactions":[{"category":"receive","txid":txid,"vout":0,"amount":1.0,"blockheight":10,"address":address,"parent_descs":[desc.as_descriptor().to_string()]}]}),
        ),
        ("gettransaction", transaction.clone()),
        ("gettxout", Json::Null),
        ("gettransaction", transaction.clone()),
        ("getblockchaininfo", chain),
    ];
    script.splice(
        6..7,
        vec![
            ("gettransaction", transaction.clone()),
            (
                "getblockhash",
                serde_json::json!(bitcoin::BlockHash::from_byte_array([8; 32])),
            ),
            (
                "listsinceblock",
                serde_json::json!({"transactions":[{"category":"send","txid":spendid}]}),
            ),
            ("gettransaction", spent_transaction.clone()),
            ("gettransaction", spent_transaction.clone()),
            ("gettransaction", transaction.clone()),
            ("gettransaction", spent_transaction.clone()),
        ],
    );
    let transactions: std::collections::HashMap<bitcoin::Txid, Json> =
        IntoIterator::into_iter([(txid, transaction), (spendid, spent_transaction)]).collect();
    let script: Vec<_> = script
        .into_iter()
        .map(|(method, value)| {
            (
                method,
                if method == "gettransaction" {
                    Reply::Transactions(transactions.clone())
                } else {
                    Reply::Result(value)
                },
            )
        })
        .collect();
    let (stop, stopped) = std::sync::mpsc::channel();
    let node = scripted_node(listener, script, stopped);
    let ticket = control.bitcoin.request_wallet_record_replay().unwrap();
    crate::bitcoin::poller::test_poll(
        &mut control.bitcoin,
        &control.db,
        &control.secp,
        &[
            control
                .config
                .main_descriptor
                .receive_descriptor()
                .as_descriptor()
                .clone(),
            control
                .config
                .main_descriptor
                .change_descriptor()
                .as_descriptor()
                .clone(),
        ],
        &Default::default(),
        &Default::default(),
    );
    stop.send(()).unwrap();
    node.join().unwrap();
    assert_eq!(
        control.bitcoin.wallet_record_replay_pending(),
        None,
        "ticket {ticket} was not acknowledged"
    );
    let coins = control.list_coins(&[], &[]).coins;
    assert_eq!(coins.len(), 1);
    assert_eq!(coins[0].block_height, Some(10));
    assert_eq!(coins[0].outpoint.txid, txid);
    assert_eq!(coins[0].spend_info.as_ref().unwrap().txid, spendid);
    assert_eq!(coins[0].spend_info.as_ref().unwrap().height, Some(11));
    assert_eq!(
        control.list_transactions(&[txid]).transactions[0].tx,
        funding
    );
    assert_eq!(control.db.connection().chain_tip(), Some(tip));
    assert_eq!(
        control.list_transactions(&[spendid]).transactions[0].tx,
        spender
    );
    assert_eq!(u32::from(coins[0].derivation_index), 500);
    drop(control);
    std::fs::remove_dir_all(root).unwrap();
}

// One bounded HTTP exchange. Matching the request ID exercises the real RPC
// decoder; tests do not replace BitcoinD's transport or classify fake errors.
fn response<T>(
    method: &'static str,
    result: Json,
    error: Json,
    call: impl FnOnce(&BitcoinD) -> T,
) -> T {
    responses(vec![(method, result, error)], |bit| call(bit))
}

fn responses<T>(
    exchanges: Vec<(&'static str, Json, Json)>,
    call: impl FnOnce(&mut BitcoinD) -> T,
) -> T {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut bit = backend(listener.local_addr().unwrap());
    let worker = thread::spawn(move || {
        for (method, result, error) in exchanges {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(e)
                        if e.kind() == io::ErrorKind::WouldBlock && Instant::now() < deadline =>
                    {
                        thread::sleep(Duration::from_millis(5))
                    }
                    other => panic!("RPC test accept failed: {:?}", other),
                }
            };
            // Accepted sockets can inherit the listener's nonblocking mode.
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut data = Vec::new();
            let mut byte = [0; 1];
            while !data.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut byte).unwrap();
                data.push(byte[0]);
                assert!(data.len() < 65536);
            }
            let head = String::from_utf8(data).unwrap();
            let len: usize = head
                .lines()
                .find_map(|line| {
                    let (key, value) = line.split_once(':')?;
                    key.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse().unwrap())
                })
                .unwrap();
            assert!(len < 65536);
            let mut body = vec![0; len];
            stream.read_exact(&mut body).unwrap();
            let request: Json = serde_json::from_slice(&body).unwrap();
            assert_eq!(request["method"], method);
            let body =
                serde_json::json!({"result":result,"error":error,"id":request["id"]}).to_string();
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
        }
    });
    let result = call(&mut bit);
    worker.join().unwrap();
    result
}

#[test]
fn unreachable_node_is_not_absence_or_rescan_completion() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let bit = backend(listener.local_addr().unwrap());
    drop(listener);
    let txid = bitcoin::Txid::all_zeros();
    let hash = bitcoin::BlockHash::all_zeros();
    assert!(bit.try_is_in_mempool(&txid).is_err());
    assert!(bit.try_get_transaction(&txid).is_err());
    assert!(bit
        .try_is_spent(&bitcoin::OutPoint { txid, vout: 0 })
        .is_err());
    assert!(bit.try_rescan_progress().is_err());
    assert!(bit.try_list_since_block(&hash).is_err());
    assert!(bit.try_chain_tip().is_err());
    assert!(bit.try_sync_progress().is_err());
    assert!(bit.try_block_time(&hash).is_err());
}

#[test]
fn polling_transport_errors_make_one_attempt_per_read() {
    // Closed-port connection timing differs across operating systems. Count
    // real accepted connections instead of timing seven connection refusals.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut bit = backend(listener.local_addr().unwrap());
    // A regression into the generic retry path must fail without making the
    // test spend a minute on each of the seven reads.
    bit.retries = 2;
    let (stop, stopped) = std::sync::mpsc::channel();
    let worker = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut requests = 0;
        loop {
            match listener.accept() {
                Ok((stream, _)) => {
                    requests += 1;
                    // Closing before an HTTP reply exercises a real retryable
                    // transport failure, not an RPC-level absence result.
                    drop(stream);
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    if stopped.try_recv().is_ok() {
                        return requests;
                    }
                    assert!(
                        Instant::now() < deadline,
                        "polling transport did not finish"
                    );
                    thread::sleep(Duration::from_millis(5));
                }
                other => panic!("RPC test accept failed: {:?}", other),
            }
        }
    });
    let txid = bitcoin::Txid::all_zeros();
    let hash = bitcoin::BlockHash::all_zeros();
    let failures = [
        bit.try_is_in_mempool(&txid).is_err(),
        bit.try_get_transaction(&txid).is_err(),
        bit.try_is_spent(&bitcoin::OutPoint { txid, vout: 0 })
            .is_err(),
        bit.try_rescan_progress().is_err(),
        bit.try_list_since_block(&hash).is_err(),
        bit.try_chain_tip().is_err(),
        bit.try_sync_progress().is_err(),
        bit.try_block_time(&hash).is_err(),
    ];
    stop.send(()).unwrap();
    let requests = worker.join().unwrap();
    assert!(failures.iter().all(|failed| *failed));
    assert_eq!(requests, 8, "polling must not retry a transport failure");
}

/// The poller's tip time is the header time of the very block it holds.
#[test]
fn block_time_reads_the_header_of_the_given_block() {
    let hash = bitcoin::BlockHash::all_zeros();
    let time = response(
        "getblockheader",
        serde_json::json!({
            "confirmations": 1,
            "height": 900_000,
            "time": 1_700_000_000u32,
            "mediantime": 1_699_999_000u32,
        }),
        Json::Null,
        |bit| bit.try_block_time(&hash),
    );
    assert_eq!(time, Ok(1_700_000_000));
}

#[test]
fn rpc_absence_is_distinct_from_operational_failure() {
    let txid = bitcoin::Txid::all_zeros();
    for code in [-5, -28, -1] {
        let err = serde_json::json!({"code":code,"message":"synthetic RPC error"});
        let mempool = response("getmempoolentry", Json::Null, err.clone(), |bit| {
            bit.try_is_in_mempool(&txid)
        });
        let transaction = response("gettransaction", Json::Null, err, |bit| {
            bit.try_get_transaction(&txid)
        });
        if code == -5 {
            assert!(!mempool.unwrap());
            assert!(transaction.unwrap().is_none());
        } else {
            assert!(mempool.is_err());
            assert!(transaction.is_err());
        }
    }
}

#[test]
fn only_explicit_idle_rescan_status_means_completion() {
    let idle = response(
        "getwalletinfo",
        serde_json::json!({"scanning":false}),
        Json::Null,
        BitcoinD::try_rescan_progress,
    );
    assert_eq!(idle.unwrap(), None);
    let active = response(
        "getwalletinfo",
        serde_json::json!({"scanning":{"progress":0.5}}),
        Json::Null,
        BitcoinD::try_rescan_progress,
    );
    assert_eq!(active.unwrap(), Some(0.5));
    for value in [
        serde_json::json!({}),
        serde_json::json!({"scanning":true}),
        serde_json::json!({"scanning":{"progress":2}}),
    ] {
        assert!(response(
            "getwalletinfo",
            value,
            Json::Null,
            BitcoinD::try_rescan_progress
        )
        .is_err());
    }
}

#[test]
fn a_hung_poll_is_bounded_and_shutdown_prevents_followup_reads() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let bit = backend(listener.local_addr().unwrap());
    let abort = bit.poll_abort.clone();
    let (accepted, acceptance) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel();
    let worker = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        let stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock && Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(5))
                }
                other => panic!("hung RPC test accept failed: {:?}", other),
            }
        };
        accepted.send(()).unwrap();
        let _ = released.recv_timeout(Duration::from_secs(POLL_RPC_SOCKET_TIMEOUT + 5));
        drop(stream);
    });
    let started = Instant::now();
    let request = thread::spawn(move || {
        let result = bit.try_chain_tip();
        (bit, result)
    });
    acceptance.recv_timeout(Duration::from_secs(5)).unwrap();
    abort.store(true, std::sync::atomic::Ordering::Relaxed);
    let (bit, result) = request.join().unwrap();
    assert!(result.unwrap_err().contains("cancelled"));
    assert!(started.elapsed() < Duration::from_secs(POLL_RPC_SOCKET_TIMEOUT + 3));
    assert!(matches!(
        bit.poll_node("getblockchaininfo", None),
        Err(BitcoindError::PollAborted)
    ));
    release.send(()).unwrap();
    worker.join().unwrap();
}

#[test]
fn malformed_successful_wallet_responses_are_errors_not_panics() {
    let txid = bitcoin::Txid::all_zeros();
    let hash = bitcoin::BlockHash::all_zeros();
    for value in [
        serde_json::json!({}),
        serde_json::json!({"hex":"zz", "confirmations":0}),
        serde_json::json!({"hex":"00", "confirmations":0}),
    ] {
        assert!(response("gettransaction", value, Json::Null, |bit| bit
            .try_get_transaction(&txid))
        .is_err());
    }
    for value in [
        serde_json::json!({}),
        serde_json::json!({"transactions":[{}]}),
        serde_json::json!({"transactions":[{"category":"receive","txid":"bad"}]}),
    ] {
        assert!(response("listsinceblock", value, Json::Null, |bit| bit
            .try_list_since_block(&hash))
        .is_err());
    }
    assert!(response(
        "listsinceblock",
        serde_json::json!({"transactions":[]}),
        Json::Null,
        |bit| bit.try_list_since_block(&hash)
    )
    .unwrap()
    .received_coins
    .is_empty());
}

#[test]
fn assumed_confirmations_are_neither_confirmed_nor_expired() {
    use crate::bitcoin::BitcoinInterface;
    let tx = bitcoin::blockdata::constants::genesis_block(bitcoin::Network::Bitcoin)
        .txdata
        .remove(0);
    let txid = tx.compute_txid();
    let op = bitcoin::OutPoint { txid, vout: 0 };
    let value = serde_json::json!({
        "hex": bitcoin::consensus::encode::serialize_hex(&tx),
        "confirmations": 0,
        "confirmations_assumed": 150,
        "generated": true,
        "blockhash": bitcoin::BlockHash::all_zeros(),
        "blockheight": 100,
        "blocktime": 123
    });
    let parsed = response("gettransaction", value.clone(), Json::Null, |bit| {
        bit.try_get_transaction(&txid)
    })
    .unwrap()
    .unwrap();
    assert!(parsed.has_assumed_confirmation());
    assert_eq!(parsed.confirmations, 0);
    let (confirmed, expired) = response("gettransaction", value.clone(), Json::Null, |bit| {
        bit.try_confirmed_coins(&[op])
    })
    .unwrap();
    assert!(confirmed.is_empty() && expired.is_empty());
    let (spent, expired) = response("gettransaction", value.clone(), Json::Null, |bit| {
        bit.try_spent_coins(&[(op, txid)])
    })
    .unwrap();
    assert!(spent.is_empty() && expired.is_empty());
    let (_, block) = response("gettransaction", value.clone(), Json::Null, |bit| {
        bit.try_wallet_transaction(&txid)
    })
    .unwrap()
    .unwrap();
    assert!(block.is_none());
    for assumed in [Json::Null, serde_json::json!(0), serde_json::json!(-1)] {
        let mut invalid = value.clone();
        invalid["confirmations_assumed"] = assumed;
        assert!(parse::transaction(invalid, txid).is_err());
    }
    let mut validated = value;
    validated["confirmations"] = serde_json::json!(150);
    validated
        .as_object_mut()
        .unwrap()
        .remove("confirmations_assumed");
    let (confirmed, expired) = response("gettransaction", validated, Json::Null, |bit| {
        bit.try_confirmed_coins(&[op])
    })
    .unwrap();
    assert_eq!(confirmed, vec![(op, 100, 123)]);
    assert!(expired.is_empty());
}

#[test]
fn an_assumed_conflicting_spend_does_not_expire_the_coin() {
    use crate::bitcoin::BitcoinInterface;
    let mut tx = bitcoin::blockdata::constants::genesis_block(bitcoin::Network::Bitcoin)
        .txdata
        .remove(0);
    let op = bitcoin::OutPoint {
        txid: bitcoin::Txid::all_zeros(),
        vout: 1,
    };
    tx.input[0].previous_output = op;
    let txid = tx.compute_txid();
    let mut conflict = tx.clone();
    conflict.lock_time = bitcoin::absolute::LockTime::from_consensus(1);
    let conflict_id = conflict.compute_txid();
    let pending = serde_json::json!({
        "hex": bitcoin::consensus::encode::serialize_hex(&tx),
        "confirmations": 0, "walletconflicts": [conflict_id]
    });
    let assumed = serde_json::json!({
        "hex": bitcoin::consensus::encode::serialize_hex(&conflict),
        "confirmations": 0, "confirmations_assumed": 150,
        "blockhash": bitcoin::BlockHash::all_zeros(),
        "blockheight": 100, "blocktime": 123
    });
    let (spent, expired) = responses(
        vec![
            ("gettransaction", pending.clone(), Json::Null),
            ("gettransaction", assumed.clone(), Json::Null),
        ],
        |bit| bit.try_spent_coins(&[(op, txid)]),
    )
    .unwrap();
    assert!(spent.is_empty() && expired.is_empty());
    let mut validated = assumed;
    validated["confirmations"] = serde_json::json!(150);
    validated
        .as_object_mut()
        .unwrap()
        .remove("confirmations_assumed");
    let (spent, expired) = responses(
        vec![
            ("gettransaction", pending, Json::Null),
            ("gettransaction", validated, Json::Null),
        ],
        |bit| bit.try_spent_coins(&[(op, txid)]),
    )
    .unwrap();
    assert_eq!(spent, vec![(op, conflict_id, 100, 123)]);
    assert!(expired.is_empty());
}

/// How `scripted_node` answers one RPC.
#[derive(Clone)]
enum Reply {
    /// A successful call returning this `result`.
    Result(Json),
    /// The node's JSON-RPC error reply, sent as bitcoind does: HTTP 500 with
    /// `{"result": null, "error": {"code": .., "message": ..}}` (#597).
    Error(i64, &'static str),
    Transactions(std::collections::HashMap<bitcoin::Txid, Json>),
}

impl From<Json> for Reply {
    fn from(result: Json) -> Reply {
        Reply::Result(result)
    }
}

// bitcoind's `getmempoolentry` reply for a transaction not in its mempool
// (`RPC_INVALID_ADDRESS_OR_KEY`).
fn not_in_mempool() -> Reply {
    Reply::Error(-5, "Transaction not in mempool")
}

// A node that answers `script` in order, then goes away: every later connection
// is closed before an HTTP reply (a real retryable transport failure) until
// `stop` fires. Unlike `responses`, a reply the client no longer reads (the
// fire-and-forget `importdescriptors`) is not an error.
fn scripted_node<R: Into<Reply>>(
    listener: TcpListener,
    script: Vec<(&'static str, R)>,
    stop: std::sync::mpsc::Receiver<()>,
) -> thread::JoinHandle<()> {
    let script: Vec<(&'static str, Reply)> = script
        .into_iter()
        .map(|(method, reply)| (method, reply.into()))
        .collect();
    // The stop signal and deadline are only checked between connections, so
    // accept must not block. Set it on this handle: a `try_clone`d Windows
    // socket does not keep the original's nonblocking mode, and a blocking
    // accept here hung the Windows CI job after the command had returned.
    listener.set_nonblocking(true).unwrap();
    thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(120);
        let mut script = script.into_iter();
        loop {
            let mut stream = match listener.accept() {
                Ok((stream, _)) => stream,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    if stop.try_recv().is_ok() {
                        return;
                    }
                    assert!(Instant::now() < deadline, "scripted node was never stopped");
                    thread::sleep(Duration::from_millis(5));
                    continue;
                }
                other => panic!("RPC test accept failed: {:?}", other),
            };
            let Some((method, reply)) = script.next() else {
                drop(stream);
                continue;
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut data = Vec::new();
            let mut byte = [0; 1];
            while !data.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut byte).unwrap();
                data.push(byte[0]);
                assert!(data.len() < 65536);
            }
            let head = String::from_utf8(data).unwrap();
            let len: usize = head
                .lines()
                .find_map(|line| {
                    let (key, value) = line.split_once(':')?;
                    key.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse().unwrap())
                })
                .unwrap();
            let mut body = vec![0; len];
            stream.read_exact(&mut body).unwrap();
            let request: Json = serde_json::from_slice(&body).unwrap();
            assert_eq!(request["method"], method, "unexpected RPC order");
            let (status, body) = match reply {
                Reply::Transactions(transactions) => (
                    "200 OK",
                    serde_json::json!({"result":transactions.get(&request["params"][0].as_str().unwrap().parse::<bitcoin::Txid>().unwrap()).unwrap(),"error":null,"id":request["id"]}),
                ),
                Reply::Result(result) => (
                    "200 OK",
                    serde_json::json!({"result":result,"error":null,"id":request["id"]}),
                ),
                Reply::Error(code, message) => (
                    "500 Internal Server Error",
                    serde_json::json!({"result":null,"error":{"code":code,"message":message},"id":request["id"]}),
                ),
            };
            let body = body.to_string();
            let _ = write!(stream, "HTTP/1.1 {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", status, body.len(), body);
        }
    })
}

/// A daemon whose commands reach `addr` through the real bitcoind backend. The
/// poller was started against a dummy backend and is already shut down, so the
/// scripted node sees the command's reads and nothing else.
fn command_control(addr: SocketAddr) -> crate::DaemonControl {
    use crate::testutils::{DummyBitcoind, DummyCoincube, DummyDatabase};
    let daemon = DummyCoincube::new(DummyBitcoind::new(), DummyDatabase::new());
    let mut control = daemon.control().clone();
    daemon.shutdown();
    let mut bit = backend(addr);
    // One attempt per read: an outage "outlasting the retry budget" without
    // spending a minute per read.
    bit.retries = 0;
    control.bitcoin = std::sync::Arc::new(std::sync::Mutex::new(bit));
    control
}

fn rescan_against(
    control: &mut crate::DaemonControl,
    listener: &TcpListener,
    script: Vec<(&'static str, Json)>,
    timestamp: u32,
) -> Result<(), crate::commands::CommandError> {
    let (stop, stopped) = std::sync::mpsc::channel();
    let node = scripted_node(listener.try_clone().unwrap(), script, stopped);
    let result = control.start_rescan(timestamp);
    stop.send(()).unwrap();
    node.join().unwrap();
    result
}

#[test]
fn start_rescan_node_outage_is_a_retryable_error_not_a_panic() {
    use crate::commands::CommandError;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut control = command_control(listener.local_addr().unwrap());

    let genesis = bitcoin::blockdata::constants::genesis_block(bitcoin::Network::Bitcoin);
    let genesis_hash = genesis.block_hash();
    let tip_hash = bitcoin::BlockHash::from_byte_array([1; 32]);
    let (genesis_time, tip_time, rescan_time) = (1_231_006_505u32, 1_700_000_000u32, 1_600_000_000);
    let header = |height: i32, time: u32| serde_json::json!({"confirmations":1,"height":height,"time":time,"mediantime":time});
    // An unpruned node: no `pruneheight`.
    let chain_info = serde_json::json!({"bestblockhash":tip_hash,"blocks":200});
    let descriptors = |timestamp: u32| {
        let desc = &control.config.main_descriptor;
        let entries: Vec<Json> = [desc.receive_descriptor(), desc.change_descriptor()]
            .iter()
            .map(|d| {
                serde_json::json!({
                    "desc": d.as_descriptor().to_string(),
                    "timestamp": timestamp,
                    "range": [0, 999],
                })
            })
            .collect();
        serde_json::json!({ "descriptors": entries })
    };
    // Every read `start_rescan` makes against bitcoind, in order.
    let script = vec![
        ("getblockhash", serde_json::json!(genesis_hash)),
        ("getblockheader", header(0, genesis_time)),
        ("getblockchaininfo", chain_info.clone()), // tip_time
        ("getblockheader", header(200, tip_time)),
        ("getwalletinfo", serde_json::json!({"scanning": false})),
        ("listdescriptors", descriptors(genesis_time)), // import range
        ("getblockchaininfo", chain_info),              // prune check
        ("importdescriptors", serde_json::json!([{"success": true}])),
        ("listdescriptors", descriptors(rescan_time)), // import confirmed
    ];

    // The node drops before each read in turn. Every such outage must come
    // back to the caller as an error, with no rescan recorded, where the reads
    // after the genesis lookup used to panic the daemon.
    for answered in 0..script.len() {
        let result = rescan_against(
            &mut control,
            &listener,
            script[..answered].to_vec(),
            rescan_time,
        );
        match (answered, result) {
            (0..=1, Err(CommandError::RescanGenesis(_))) => {}
            (2.., Err(CommandError::RescanTrigger(_))) => {}
            (answered, result) => {
                panic!(
                    "outage after {} answered reads returned {:?}",
                    answered, result
                )
            }
        }
        assert!(control.db.connection().rescan_timestamp().is_none());
    }

    // The node is back: the same command now succeeds and records the rescan.
    rescan_against(&mut control, &listener, script, rescan_time).unwrap();
    assert_eq!(
        control.db.connection().rescan_timestamp(),
        Some(rescan_time)
    );
}

#[test]
fn command_reads_during_a_node_outage_do_not_panic_or_claim_completion() {
    use crate::commands::CommandError;
    use std::str::FromStr;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let control = command_control(listener.local_addr().unwrap());
    drop(listener);

    // A pending rescan the node cannot report on is still pending.
    control.db.connection().set_rescan(1_600_000_000);
    let progress = control.get_info().rescan_progress.unwrap();
    assert!(
        progress < 1.0,
        "an outage reported rescan progress {}",
        progress
    );

    let address = bitcoin::Address::from_str("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4").unwrap();
    assert!(matches!(
        control.create_recovery(address, &[], 2, None),
        Err(CommandError::ChainTipUnavailable(_))
    ));
}

/// Run `call` while `scripted_node` answers `script` on `listener`.
fn against_node<T, R: Into<Reply>>(
    listener: &TcpListener,
    script: Vec<(&'static str, R)>,
    call: impl FnOnce() -> T,
) -> T {
    let (stop, stopped) = std::sync::mpsc::channel();
    let node = scripted_node(listener.try_clone().unwrap(), script, stopped);
    let result = call();
    stop.send(()).unwrap();
    node.join().unwrap();
    result
}

/// A wallet coin of `amount` sats paid to our receive index 0, with its funding
/// transaction stored so a spend of it can be built.
fn store_coin(
    control: &crate::DaemonControl,
    amount: u64,
    block_info: Option<crate::database::BlockInfo>,
    is_from_self: bool,
) -> bitcoin::OutPoint {
    use bitcoin::{absolute, transaction::Version};
    let script_pubkey = control
        .config
        .main_descriptor
        .receive_descriptor()
        .derive(0)
        .unwrap()
        .script_pubkey();
    let funding = bitcoin::Transaction {
        version: Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: vec![bitcoin::TxIn {
            previous_output: bitcoin::OutPoint::null(),
            script_sig: bitcoin::ScriptBuf::from_bytes(vec![0x51]),
            sequence: bitcoin::Sequence::MAX,
            witness: bitcoin::Witness::default(),
        }],
        output: vec![bitcoin::TxOut {
            value: bitcoin::Amount::from_sat(amount),
            script_pubkey,
        }],
    };
    let outpoint = bitcoin::OutPoint::new(funding.compute_txid(), 0);
    let mut db_conn = control.db.connection();
    db_conn.new_txs(std::slice::from_ref(&funding));
    db_conn.new_unspent_coins(&[crate::database::Coin {
        outpoint,
        is_immature: false,
        block_info,
        amount: bitcoin::Amount::from_sat(amount),
        derivation_index: 0.into(),
        is_change: false,
        spend_txid: None,
        spend_block: None,
        is_from_self,
    }]);
    outpoint
}

// `getmempoolentry` for a 141 vB transaction paying 1 sat/vB, alone in the mempool.
fn mempool_entry_json() -> Json {
    serde_json::json!({
        "vsize": 141,
        "ancestorsize": 141,
        "fees": {"base": 0.00000141, "modified": 0.00000141, "ancestor": 0.00000141, "descendant": 0.00000141},
    })
}

// The reads the anti-fee-sniping locktime makes once a spend is built (#589).
fn locktime_reads() -> Vec<(&'static str, Json)> {
    let chain_info = serde_json::json!({"bestblockhash": bitcoin::BlockHash::from_byte_array([1; 32]), "blocks": 200});
    vec![
        ("getblockchaininfo", chain_info.clone()),
        ("getblockchaininfo", chain_info),
        (
            "getblockheader",
            serde_json::json!({"confirmations":1,"height":200,"time":1_700_000_000u32,"mediantime":1_700_000_000u32}),
        ),
    ]
}

#[test]
fn createspend_mempool_outage_is_a_retryable_error_not_a_panic() {
    use crate::commands::{CommandError, CreateSpendResult};
    use std::{collections::HashMap, str::FromStr};
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let control = command_control(listener.local_addr().unwrap());
    // An unconfirmed coin from one of our own transactions: coin selection
    // asks the node for its ancestors, whether we pick it or it is picked.
    let outpoint = store_coin(&control, 100_000, None, true);
    let destination =
        bitcoin::Address::from_str("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4").unwrap();
    let destinations = HashMap::from([(destination, 50_000)]);
    let change_index = control.db.connection().change_index();

    for coins in [vec![], vec![outpoint]] {
        // The node is down for the ancestor read: an error the caller can retry,
        // no change address consumed, and the backend lock still usable.
        let result = against_node(&listener, Vec::<(_, Json)>::new(), || {
            control.create_spend(&destinations, &coins, 1, None)
        });
        assert!(
            matches!(result, Err(CommandError::MempoolUnavailable(_))),
            "outage with coins {:?} returned {:?}",
            coins,
            result
        );
        assert!(!control.bitcoin.is_poisoned());
        assert_eq!(control.db.connection().change_index(), change_index);
    }

    // The node is back: the same command succeeds.
    let mut script = vec![("getmempoolentry", mempool_entry_json())];
    script.extend(locktime_reads());
    let result = against_node(&listener, script, || {
        control.create_spend(&destinations, &[outpoint], 1, None)
    });
    assert!(
        matches!(result, Ok(CreateSpendResult::Success { .. })),
        "{:?}",
        result
    );
}

/// A stored, unconfirmed 1 sat/vB spend of a confirmed wallet coin, ready to be
/// replaced: the spent coin's outpoint and the spend's txid.
fn stored_replaceable_spend(
    control: &crate::DaemonControl,
    listener: &TcpListener,
) -> (bitcoin::OutPoint, bitcoin::Txid) {
    use crate::commands::CreateSpendResult;
    use std::{collections::HashMap, str::FromStr};
    // A confirmed coin needs no mempool read to spend.
    let outpoint = store_coin(
        control,
        100_000,
        Some(crate::database::BlockInfo {
            height: 100,
            time: 1_600_000_000,
        }),
        false,
    );
    let destination =
        bitcoin::Address::from_str("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4").unwrap();
    let psbt = match against_node(listener, locktime_reads(), || {
        control.create_spend(
            &HashMap::from([(destination, 50_000)]),
            &[outpoint],
            1,
            None,
        )
    }) {
        Ok(CreateSpendResult::Success { psbt, .. }) => psbt,
        other => panic!("could not build the spend to replace: {:?}", other),
    };
    let txid = psbt.unsigned_tx.compute_txid();
    let mut db_conn = control.db.connection();
    db_conn.store_spend(&psbt);
    db_conn.spend_coins(&[(outpoint, txid)]);
    (outpoint, txid)
}

#[test]
fn rbfpsbt_mempool_outage_is_a_retryable_error_not_a_panic() {
    use crate::commands::{CommandError, CreateSpendResult};
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let control = command_control(listener.local_addr().unwrap());
    let (outpoint, txid) = stored_replaceable_spend(&control, &listener);
    let change_index = control.db.connection().change_index();

    // Every read `rbfpsbt` makes against bitcoind, in order.
    let mut script = vec![
        (
            "gettxspendingprevout",
            serde_json::json!([{"txid": outpoint.txid.to_string(), "vout": outpoint.vout, "spendingtxid": txid.to_string()}]),
        ),
        ("getmempoolentry", mempool_entry_json()),
    ];
    // The node drops before each mempool read in turn. An outage is not "the
    // replaced transaction left the mempool": it must come back as an error,
    // with the backend lock still usable, where it used to panic the daemon.
    for answered in 0..script.len() {
        let result = against_node(&listener, script[..answered].to_vec(), || {
            control.rbf_psbt(&txid, true, None)
        });
        assert!(
            matches!(result, Err(CommandError::MempoolUnavailable(_))),
            "outage after {} answered reads returned {:?}",
            answered,
            result
        );
        assert!(!control.bitcoin.is_poisoned());
        assert_eq!(control.db.connection().change_index(), change_index);
    }

    // The node is back: the same command succeeds, above the replaced feerate.
    script.extend(locktime_reads());
    let psbt = match against_node(&listener, script, || control.rbf_psbt(&txid, true, None)) {
        Ok(CreateSpendResult::Success { psbt, .. }) => psbt,
        other => panic!("replacement failed once the node was back: {:?}", other),
    };
    let fee = psbt.fee().unwrap().to_sat();
    let vsize = psbt.unsigned_tx.vsize() as u64;
    assert!(fee >= 2 * vsize, "fee {} for {} vB", fee, vsize);
}

// The mempool reads of a replacement: `outpoint` is spent by `txid` in the
// node's mempool, whose entry the node answers with `entry`.
fn rbf_mempool_reads(
    outpoint: bitcoin::OutPoint,
    txid: bitcoin::Txid,
    entry: Reply,
) -> Vec<(&'static str, Reply)> {
    vec![
        (
            "gettxspendingprevout",
            serde_json::json!([{"txid": outpoint.txid.to_string(), "vout": outpoint.vout, "spendingtxid": txid.to_string()}]).into(),
        ),
        ("getmempoolentry", entry),
    ]
}

fn replies(script: Vec<(&'static str, Json)>) -> Vec<(&'static str, Reply)> {
    script
        .into_iter()
        .map(|(method, result)| (method, result.into()))
        .collect()
}

// Other JSON-RPC errors `getmempoolentry` could answer with: none of them says
// the transaction is absent.
const NOT_ABSENCE: [(i64, &str); 3] = [
    (-1, "misc error"),
    (-8, "invalid parameter"),
    (-32603, "internal error"),
];

#[test]
fn createspend_not_in_mempool_reply_is_absence_and_other_rpc_errors_are_errors() {
    use crate::commands::{CommandError, CreateSpendResult};
    use std::{collections::HashMap, str::FromStr};
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let control = command_control(listener.local_addr().unwrap());
    // An unconfirmed coin from one of our own transactions: coin selection asks
    // the node for its ancestors.
    let outpoint = store_coin(&control, 100_000, None, true);
    let destination =
        bitcoin::Address::from_str("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4").unwrap();
    let destinations = HashMap::from([(destination, 50_000)]);
    let change_index = control.db.connection().change_index();

    for coins in [vec![], vec![outpoint]] {
        // Any other error reply is not "not in the mempool": the command fails
        // with a retryable error and consumes nothing.
        for (code, message) in NOT_ABSENCE {
            let result = against_node(
                &listener,
                vec![("getmempoolentry", Reply::Error(code, message))],
                || control.create_spend(&destinations, &coins, 1, None),
            );
            assert!(
                matches!(result, Err(CommandError::MempoolUnavailable(_))),
                "RPC error {} with coins {:?} returned {:?}",
                code,
                coins,
                result
            );
            assert!(!control.bitcoin.is_poisoned());
            assert_eq!(control.db.connection().change_index(), change_index);
        }
    }

    // bitcoind's -5 reply is the node saying the transaction left its mempool:
    // the coin is used without ancestor info, as before #594.
    for coins in [vec![], vec![outpoint]] {
        let mut script = vec![("getmempoolentry", not_in_mempool())];
        script.extend(replies(locktime_reads()));
        let result = against_node(&listener, script, || {
            control.create_spend(&destinations, &coins, 1, None)
        });
        assert!(
            matches!(result, Ok(CreateSpendResult::Success { .. })),
            "-5 with coins {:?} returned {:?}",
            coins,
            result
        );
    }
}

#[test]
fn rbfpsbt_spender_not_in_mempool_falls_back_to_min_feerate_and_other_rpc_errors_are_errors() {
    use crate::commands::{CommandError, CreateSpendResult};
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let control = command_control(listener.local_addr().unwrap());
    let (outpoint, txid) = stored_replaceable_spend(&control, &listener);
    let change_index = control.db.connection().change_index();

    // An error reply to either read that does not say "absent" is an error the
    // caller can retry, not a replacement that drops the RBF minimums.
    let mut scripts = vec![vec![(
        "gettxspendingprevout",
        Reply::Error(-8, "invalid parameter"),
    )]];
    for (code, message) in NOT_ABSENCE {
        scripts.push(rbf_mempool_reads(
            outpoint,
            txid,
            Reply::Error(code, message),
        ));
    }
    for script in scripts {
        let result = against_node(&listener, script, || control.rbf_psbt(&txid, true, None));
        assert!(
            matches!(result, Err(CommandError::MempoolUnavailable(_))),
            "{:?}",
            result
        );
        assert!(!control.bitcoin.is_poisoned());
        assert_eq!(control.db.connection().change_index(), change_index);
    }

    // The spender left the mempool between the two reads (-5): there is no
    // replaced feerate to beat, so the cancel falls back to the minimum of 1 sat/vB.
    let mut script = rbf_mempool_reads(outpoint, txid, not_in_mempool());
    script.extend(replies(locktime_reads()));
    let psbt = match against_node(&listener, script, || control.rbf_psbt(&txid, true, None)) {
        Ok(CreateSpendResult::Success { psbt, .. }) => psbt,
        other => panic!("-5 for the spender returned {:?}", other),
    };
    let fee = psbt.fee().unwrap().to_sat();
    let vsize = psbt.unsigned_tx.vsize() as u64;
    assert!(
        fee >= vsize && fee < 2 * vsize,
        "fee {} for {} vB",
        fee,
        vsize
    );
}

// `getmempoolentry` answers a coin-selection or replacement read cannot use.
fn unusable_mempool_entries() -> Vec<Json> {
    let good = mempool_entry_json();
    let mut entries = Vec::new();
    for (field, value) in [
        ("vsize", serde_json::json!(0)),
        ("vsize", serde_json::json!("141")),
        ("ancestorsize", Json::Null),
        ("fees", serde_json::json!(141)),
    ] {
        let mut bad = good.clone();
        bad[field] = value;
        entries.push(bad);
    }
    for field in ["base", "ancestor", "descendant"] {
        let mut bad = good.clone();
        bad["fees"][field] = serde_json::json!(-0.00000141);
        entries.push(bad);
        let mut bad = good.clone();
        bad["fees"].as_object_mut().unwrap().remove(field);
        entries.push(bad);
    }
    entries
}

#[test]
fn malformed_mempool_entry_is_an_error_not_a_panic() {
    use crate::commands::CommandError;
    use std::{collections::HashMap, str::FromStr};
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let control = command_control(listener.local_addr().unwrap());
    let (outpoint, txid) = stored_replaceable_spend(&control, &listener);
    let unconfirmed = store_coin(&control, 200_000, None, true);
    let destination =
        bitcoin::Address::from_str("bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4").unwrap();
    let destinations = HashMap::from([(destination, 50_000)]);
    let change_index = control.db.connection().change_index();

    // An ancestor fee coin selection cannot represent (more than u32::MAX sat)
    // is refused too, where it used to panic.
    let mut huge_ancestor_fee = mempool_entry_json();
    huge_ancestor_fee["fees"]["ancestor"] = serde_json::json!(43.0);
    let mut createspend_entries = unusable_mempool_entries();
    createspend_entries.push(huge_ancestor_fee);

    for entry in createspend_entries {
        let result = against_node(&listener, vec![("getmempoolentry", entry.clone())], || {
            control.create_spend(&destinations, &[unconfirmed], 1, None)
        });
        assert!(
            matches!(result, Err(CommandError::MempoolUnavailable(_))),
            "createspend with {} returned {:?}",
            entry,
            result
        );
        assert!(!control.bitcoin.is_poisoned());
        assert_eq!(control.db.connection().change_index(), change_index);
    }
    for entry in unusable_mempool_entries() {
        let result = against_node(
            &listener,
            rbf_mempool_reads(outpoint, txid, entry.clone().into()),
            || control.rbf_psbt(&txid, true, None),
        );
        assert!(
            matches!(result, Err(CommandError::MempoolUnavailable(_))),
            "rbfpsbt with {} returned {:?}",
            entry,
            result
        );
        assert!(!control.bitcoin.is_poisoned());
        assert_eq!(control.db.connection().change_index(), change_index);
    }
}

fn local_fork_snapshot(chain: coincube_core::chain::ChainId) -> Vec<(&'static str, Json, Json)> {
    let hash = "01".repeat(32);
    let (name, height) = if chain == coincube_core::chain::ChainId::BitcoinBlake2b {
        ("main", 961_640)
    } else {
        ("testnet4", 150_308)
    };
    vec![
        (
            "getblockchaininfo",
            serde_json::json!({"chain":name, "blocks":height+1, "bestblockhash":hash, "initialblockdownload":false}),
            Json::Null,
        ),
        (
            "getdeploymentinfo",
            serde_json::json!({"hash":hash, "height":height+1, "blake2b":{"height":height,"active":true}}),
            Json::Null,
        ),
        (
            "getblockheader",
            serde_json::json!({"hash":hash,"height":height+1,"header_version":2,"confirmations":1}),
            Json::Null,
        ),
        ("getbestblockhash", Json::String(hash), Json::Null),
    ]
}

#[test]
fn local_fork_admission_requires_coherent_active_tip_for_both_chains() {
    use coincube_core::chain::ChainId;
    for chain in [ChainId::BitcoinBlake2b, ChainId::BitcoinBlake2bTestnet4] {
        responses(local_fork_snapshot(chain), |bit| {
            bit.check_local_fork_chain(chain, true).unwrap()
        });
        for failure in 0..9 {
            let mut snapshot = local_fork_snapshot(chain);
            match failure {
                0 => {
                    snapshot[0].1["chain"] = Json::String("signet".into());
                    snapshot.truncate(1);
                }
                1 => {
                    snapshot[1].1["blake2b"]["height"] = Json::from(1);
                    snapshot.truncate(2);
                }
                2 => {
                    snapshot[1].1["hash"] = Json::String("02".repeat(32));
                    snapshot.truncate(2);
                }
                3 => {
                    snapshot[0].1["initialblockdownload"] = Json::Bool(true);
                    snapshot.truncate(3);
                }
                4 => {
                    snapshot[1].1["blake2b"]["active"] = Json::Bool(false);
                    snapshot.truncate(3);
                }
                5 => {
                    snapshot[2].1["header_version"] = Json::from(0);
                    snapshot.truncate(3);
                }
                6 => {
                    snapshot[2].1["height"] = Json::from(2);
                    snapshot.truncate(3);
                }
                7 => {
                    snapshot[2].1["confirmations"] = Json::from(-1);
                    snapshot.truncate(3);
                }
                _ => {
                    snapshot[3].1 = Json::String("02".repeat(32));
                }
            }
            responses(snapshot, |bit| {
                assert!(
                    bit.check_local_fork_chain(chain, true).is_err(),
                    "failure {}",
                    failure
                )
            });
        }
    }
}

#[test]
fn local_fork_schedule_allows_syncing_companion_but_not_wallet() {
    use coincube_core::chain::ChainId;
    let chain = ChainId::BitcoinBlake2b;
    let mut snapshot = local_fork_snapshot(chain);
    snapshot[0].1["initialblockdownload"] = Json::Bool(true);
    snapshot[0].1["blocks"] = Json::from(10);
    snapshot[1].1["height"] = Json::from(10);
    snapshot[1].1["blake2b"]["active"] = Json::Bool(false);
    snapshot.remove(2);
    responses(snapshot, |bit| {
        bit.check_local_fork_chain(chain, false).unwrap()
    });
    let mut snapshot = local_fork_snapshot(chain);
    snapshot[0].1.as_object_mut().unwrap().remove("blocks");
    responses(snapshot[..1].to_vec(), |bit| {
        assert!(bit.check_local_fork_chain(chain, false).is_err())
    });
}

#[test]
fn local_fork_guard_refuses_wallet_rpc_after_wrong_chain_restart() {
    use coincube_core::chain::ChainId;
    let chain = ChainId::BitcoinBlake2b;
    let mut snapshot = local_fork_snapshot(chain);
    snapshot.push((
        "getblockchaininfo",
        serde_json::json!({"chain":"main"}),
        Json::Null,
    ));
    // Missing blocks/hash on a restarted ordinary Bitcoin node: no requested
    // importdescriptors mutation is ever sent to it.
    responses(snapshot, |bit| {
        let guarded = backend(bit.config.addr).admit_local_fork(chain).unwrap();
        assert!(guarded
            .make_request_inner(
                ClientKind::Watchonly,
                "importdescriptors",
                params!(Json::Array(vec![])),
                false
            )
            .is_err());
    });
}

#[test]
fn local_fork_start_rejects_bitcoin_before_creating_wallet_files() {
    use crate::{
        config::{BitcoinBackend, BitcoinConfig, Config},
        datadir::DataDirectory,
        DaemonHandle,
    };
    use coincube_core::chain::ChainId;
    let dir = std::env::temp_dir().join(format!("btcb2-admission-no-write-{}", std::process::id()));
    assert!(!dir.exists());
    let desc = coincube_core::descriptors::CoincubeDescriptor::from_str(concat!(
        "wsh(andor(pk([aabbccdd]xpub68JJTXc1MWK8KLW4HGLXZBJknja7kDUJuFHnM424LbziEXsfkh1WQCiEjjHw4z",
        "LqSUm4rvhgyGkkuRowE9tCJSgt3TQB5J3SKAbZ2SdcKST/<0;1>/*),older(10000),pk([aabbccdd]xpub68JJT",
        "Xc1MWK8PEQozKsRatrUHXKFNkD1Cb1BuQU9Xr5moCv87anqGyXLyUd4KpnDyZgo3gz4aN1r3NiaoweFW8Uut",
        "BsBbgKHzaD5HkTkifK/<0;1>/*)))#3xh8xmhn")).unwrap();
    responses(
        vec![
            ("echo", Json::Array(vec![]), Json::Null),
            ("echo", Json::Array(vec![]), Json::Null),
            (
                "getblockchaininfo",
                serde_json::json!({"chain":"main","blocks":970000,"bestblockhash":"01".repeat(32)}),
                Json::Null,
            ),
            (
                "getdeploymentinfo",
                serde_json::json!({"height":970000,"hash":"01".repeat(32),"deployments":{}}),
                Json::Null,
            ),
        ],
        |bit| {
            let cfg = Config::new(
                BitcoinConfig::new(ChainId::BitcoinBlake2b, Duration::from_secs(10)),
                Some(BitcoinBackend::Bitcoind(bit.config.clone())),
                log::LevelFilter::Info,
                desc,
                DataDirectory::new(dir.clone()),
            );
            assert!(DaemonHandle::start_with_local_node(cfg).is_err());
        },
    );
    assert!(!dir.exists(), "refused node created wallet data");
}

#[test]
fn local_fork_tip_movement_retries_a_complete_snapshot_with_a_fixed_bound() {
    use coincube_core::chain::ChainId;
    let chain = ChainId::BitcoinBlake2b;
    let mut moving = local_fork_snapshot(chain);
    moving[3].1 = Json::String("02".repeat(32));
    let mut recovering = moving.clone();
    recovering.extend(local_fork_snapshot(chain));
    responses(recovering, |bit| {
        bit.check_local_fork_chain(chain, true).unwrap()
    });
    let repeated = (0..3).flat_map(|_| moving.clone()).collect();
    responses(repeated, |bit| {
        assert!(matches!(
            bit.check_local_fork_chain(chain, true),
            Err(BitcoindError::LocalForkTipChanged)
        ))
    });
}

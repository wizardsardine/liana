//! Actual companion -> native pairing/IPC -> wallet preparation -> native review.
//! Synthetic native consent, a disposable Cube root and public chain fixtures;
//! no external node, wallet installation, signing, broadcasting, OS keystore
//! or packaged host.
use super::*;
use crate::{
    app::{session, settings::CubeSettings},
    chain::ChainId,
    dir::CoincubeDirectory,
};
use serde_json::{json, Value};
use std::path::Path;

#[path = "../../../../coincube-mcp/tests/support/stdio.rs"]
mod stdio;

const CUBE: &str = "stdio-native-acceptance";
const KEY: [u8; 32] = [53; 32];

fn settings(root: &Path, chain: ChainId, create: bool) -> Settings {
    std::fs::create_dir_all(root.join(chain.dir_name())).unwrap();
    let location = pairings::Location::new(root, chain, CUBE).unwrap();
    Settings {
        enabled: true,
        pairing_store: Some(
            pairings::Store::open(location.clone(), zeroize::Zeroizing::new(KEY), create).unwrap(),
        ),
        pairing_location: Some(location),
        ..Settings::default()
    }
}

fn refresh(
    settings: &mut Settings,
    root: &Path,
    chain: ChainId,
    wallet: Option<Arc<Wallet>>,
    daemon: Option<Arc<dyn Daemon + Send + Sync>>,
) {
    let setup = wallet.is_none();
    settings.ensure_pairings(PairingContext {
        root,
        chain,
        cube_id: CUBE,
        wallet,
        daemon,
        supported: !setup,
        proposal_supported: setup,
        last_poll_at_startup: None,
    });
}

fn pair(
    settings: &mut Settings,
    wallet: Option<Arc<Wallet>>,
    daemon: Option<Arc<dyn Daemon + Send + Sync>>,
) -> String {
    let intent = if wallet.is_some() {
        vec![Message::Wallet(true), Message::Transactions(true)]
    } else {
        vec![Message::Creation(true)]
    };
    for message in intent.into_iter().chain([
        Message::ClientName("Synthetic acceptance client".into()),
        Message::Pair,
    ]) {
        settings.update(message, wallet.clone(), daemon.clone(), true, CUBE);
    }
    settings
        .pairing_store
        .as_ref()
        .unwrap()
        .clients()
        .last()
        .unwrap()
        .id
        .clone()
}

async fn client(settings: &mut Settings, id: &str) -> stdio::Client {
    let configuration = settings
        .update(Message::CopyPair(id.into()), None, None, true, CUBE)
        .unwrap();
    let configuration: Value = serde_json::from_str(&configuration).unwrap();
    let environment: Vec<_> = configuration["mcpServers"]["tenshu"]["env"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(key, value)| (key.clone(), value.as_str().unwrap().to_owned()))
        .collect();
    let executable = std::env::var_os("TENSHU_MCP_TEST_ADAPTER")
        .expect("build the companion and set TENSHU_MCP_TEST_ADAPTER");
    let mut client = stdio::Client::start(&executable, &environment).await;
    let discovered = client.request("tools/list", json!({})).await;
    assert_eq!(discovered["result"]["tools"].as_array().unwrap().len(), 10);
    let caps = client.call("tenshu_get_capabilities", json!({})).await;
    let caps = &caps["structuredContent"];
    assert_eq!(caps["schema_version"], 2);
    assert_eq!(caps["wallet_proposals"], true);
    assert_eq!(caps["transaction_preparation"], true);
    assert_eq!(caps["wallet_creation_via_mcp"], false);
    for support in ["proposal_support", "transaction_support"] {
        assert_eq!(
            caps[support]["networks"],
            json!(wire::Chain::ALL
                .iter()
                .map(|c| c.as_str())
                .collect::<Vec<_>>())
        );
    }
    client
}

fn denied(result: Value) {
    tool_error(result, "access_denied");
}

fn tool_error(result: Value, code: &str) {
    assert_eq!(result["isError"], true);
    assert_eq!(result["structuredContent"], json!({"error":{"code":code}}));
}

async fn invalid_schema(client: &mut stdio::Client, name: &str, arguments: Value) {
    let response = client
        .request("tools/call", json!({"name":name,"arguments":arguments}))
        .await;
    assert_eq!(response["error"]["code"], -32602);
    assert_eq!(response["error"]["message"], "invalid_input");
}

fn unavailable(result: Value) {
    assert_eq!(result["isError"], true);
    assert!(matches!(
        result["structuredContent"]["error"]["code"].as_str(),
        Some("access_denied" | "wallet_unavailable")
    ));
    assert_eq!(result["structuredContent"].as_object().unwrap().len(), 1);
}

fn summary(result: Value) -> Value {
    assert_ne!(result["isError"], true, "preparation must succeed");
    let value = result["structuredContent"].clone();
    let encoded = result.to_string();
    for forbidden in [
        "xpub",
        "tpub",
        "psbt",
        "credential",
        "private_key",
        "script_pubkey",
    ] {
        assert!(
            !encoded.contains(forbidden),
            "native material crossed the MCP boundary"
        );
    }
    value
}

fn revoke(settings: &mut Settings, id: &str) {
    settings.update(
        Message::RequestRevokePair(id.into()),
        None,
        None,
        true,
        CUBE,
    );
    settings.update(Message::RevokePair(id.into()), None, None, true, CUBE);
    assert!(!settings
        .pairing_store
        .as_ref()
        .unwrap()
        .clients()
        .iter()
        .any(|c| c.id == id));
}

fn configure_native_node(ctx: &mut crate::installer::Context, chain: ChainId) {
    use crate::node::bitcoind::*;
    ctx.bitcoin_backend = Some(coincubed::config::BitcoinBackend::Bitcoind(
        coincubed::config::BitcoindConfig {
            addr: "127.0.0.1:28443".parse().unwrap(),
            rpc_auth: coincubed::config::BitcoindRpcAuth::UserPass(
                "fixture".into(),
                "not-a-secret".into(),
            ),
        },
    ));
    if chain.is_blake2b() {
        let directory =
            internal_bitcoind_datadir_for(&ctx.coincube_directory, NodeChainFamily::BitcoinBlake2b);
        std::fs::create_dir_all(&directory).unwrap();
        let mut managed = InternalBitcoindConfig::for_flavor(NodeFlavor::KnotsBlake2b);
        managed.networks.insert(
            chain.bitcoin_network(),
            InternalBitcoindNetworkConfig {
                rpc_port: 18444,
                p2p_port: 18445,
                prune: 550,
                rpc_auth: None,
            },
        );
        managed
            .to_ini()
            .write_to_file(internal_bitcoind_config_path(&directory))
            .unwrap();
        ctx.bitcoin_backend = Some(coincubed::config::BitcoinBackend::Bitcoind(
            coincubed::config::BitcoindConfig {
                addr: "127.0.0.1:18444".parse().unwrap(),
                rpc_auth: coincubed::config::BitcoindRpcAuth::CookieFile(
                    internal_bitcoind_cookie_path(&directory, &chain.bitcoin_network()),
                ),
            },
        ));
    }
}

#[test]
#[ignore = "build tenshu-mcp and set TENSHU_MCP_TEST_ADAPTER; run explicitly in platform CI"]
fn stdio_client_creation_reaches_native_review_on_every_chain() {
    use crate::installer::{
        self,
        proposal::Review,
        step::{descriptor::editor::DefineDescriptor, Step},
        Installer, UserFlow,
    };
    use coincube_core::{miniscript::bitcoin::secp256k1::Secp256k1, signer::MasterSigner};
    let _guard = session::test_guard();
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        for network in wire::Chain::ALL {
            eprintln!(
                "MCP acceptance workflow=creation chain={}",
                network.as_str()
            );
            let chain: ChainId = network.as_str().parse().unwrap();
            let root = installer::proposal::tests::TestDir::new();
            let master = MasterSigner::generate(chain.bitcoin_network()).unwrap();
            let fingerprint = master.fingerprint(&Secp256k1::new());
            let cube = CubeSettings::new_with_raw_id(CUBE.into(), "Disposable".into(), chain)
                .with_master_signer(fingerprint);
            let mut settings = settings(root.path(), chain, true);
            std::fs::write(
                root.path().join(chain.dir_name()).join("settings.json"),
                serde_json::to_vec(&crate::app::settings::Settings {
                    cubes: vec![cube.clone()],
                    ..Default::default()
                })
                .unwrap(),
            )
            .unwrap();
            session::open_without_pin(CUBE);
            let id = pair(&mut settings, None, None);
            refresh(&mut settings, root.path(), chain, None, None);
            let mut client = client(&mut settings, &id).await;
            let input = json!(installer::creation::tests::input_for_chain(network));
            for other in wire::Chain::ALL
                .iter()
                .copied()
                .filter(|other| *other != network)
            {
                let mut wrong = input.clone();
                wrong["network"] = json!(other);
                tool_error(
                    client.call("tenshu_prepare_wallet_creation", wrong).await,
                    "invalid_input",
                );
            }
            let mut missing = input.clone();
            missing.as_object_mut().unwrap().remove("network");
            invalid_schema(&mut client, "tenshu_prepare_wallet_creation", missing).await;
            let first = summary(
                client
                    .call("tenshu_prepare_wallet_creation", input.clone())
                    .await,
            );
            assert!(
                serde_json::from_value::<wire::proposal::Proposal>(first.clone()).is_ok(),
                "proposal contains unexpected fields"
            );
            assert_eq!(first["network"], network.as_str());
            assert_eq!(first["descriptor_validated"], true);
            assert_eq!(first["state"], "ready_for_native_review");
            assert_eq!(
                summary(
                    client
                        .call("tenshu_prepare_wallet_creation", input.clone())
                        .await
                ),
                first
            );
            let lookup = json!({"proposal_id":first["proposal_id"]});
            assert_eq!(
                summary(
                    client
                        .call("tenshu_get_wallet_proposal", lookup.clone())
                        .await
                ),
                first
            );
            let mut changed = input.clone();
            changed["request_id"] = json!("changed-request");
            assert_eq!(
                client.call("tenshu_prepare_wallet_creation", changed).await["structuredContent"]
                    ["error"]["code"],
                "proposal_conflict"
            );
            denied(client.call("tenshu_list_wallets", json!({})).await);
            denied(
                client
                    .call(
                        "tenshu_get_prepared_transaction",
                        json!({"wallet_id":"foreign","draft_id":"foreign"}),
                    )
                    .await,
            );
            for tool in ["tenshu_create_wallet", "tenshu_approve_wallet"] {
                assert_eq!(
                    client
                        .request("tools/call", json!({"name":tool,"arguments":{}}))
                        .await["error"]["code"],
                    -32602
                );
            }
            // A separate setup pairing cannot retrieve the first client's draft.
            let replacement_id = pair(&mut settings, None, None);
            refresh(&mut settings, root.path(), chain, None, None);
            let mut replacement = self::client(&mut settings, &replacement_id).await;
            denied(
                replacement
                    .call("tenshu_get_wallet_proposal", lookup.clone())
                    .await,
            );
            revoke(&mut settings, &id);
            unavailable(client.call("tenshu_get_wallet_proposal", lookup).await);
            client.close().await;
            let before_lock = summary(
                replacement
                    .call("tenshu_prepare_wallet_creation", input.clone())
                    .await,
            );
            session::close();
            denied(
                replacement
                    .call(
                        "tenshu_get_wallet_proposal",
                        json!({"proposal_id":before_lock["proposal_id"]}),
                    )
                    .await,
            );
            settings.revoke();
            session::open_without_pin(CUBE);
            refresh(&mut settings, root.path(), chain, None, None);
            denied(
                replacement
                    .call(
                        "tenshu_get_wallet_proposal",
                        json!({"proposal_id":before_lock["proposal_id"]}),
                    )
                    .await,
            );
            let current = summary(
                replacement
                    .call("tenshu_prepare_wallet_creation", input.clone())
                    .await,
            );
            assert_ne!(current["proposal_id"], before_lock["proposal_id"]);

            // The native click transfers the actual STDIO-prepared session into
            // the real installer. MCP has no path to trigger this transition.
            let proposal = settings
                .paired_proposal_session(CUBE, &replacement_id)
                .unwrap();
            // Model native unlock without reopening the session: the actual
            // STDIO proposal must keep its original authorization generation.
            let generation = session::generation_for(CUBE).unwrap();
            session::store_unlocked_signer(CUBE, fingerprint, master.try_clone().unwrap());
            assert_eq!(session::generation_for(CUBE), Some(generation));
            assert!(proposal.active());
            let (mut installer, startup) = Installer::try_new_for_chain(
                CoincubeDirectory::new(root.path().to_owned()),
                chain,
                UserFlow::CreateWallet,
                true,
                Some(cube),
                None,
                None,
                false,
                None,
            )
            .unwrap();
            drop(startup);
            assert_eq!(session::generation_for(CUBE), Some(generation));
            assert!(proposal.active());
            drop(installer.use_mcp_proposal(settings, proposal).unwrap());
            let proposal = installer.context.mcp_proposal.clone().unwrap();
            assert_eq!(proposal.chain(), chain);
            let signer = Arc::new(std::sync::Mutex::new(crate::signer::Signer::new(master)));
            let mut builder = DefineDescriptor::new(chain.bitcoin_network(), signer);
            builder.load_context(&installer.context);
            assert!(builder.apply(&mut installer.context));
            let expected = installer::creation::validate(
                &installer::creation::tests::input_for_chain(network),
            )
            .unwrap();
            assert_eq!(
                installer.context.descriptor.as_ref(),
                Some(&expected.descriptor)
            );
            assert_eq!(installer.context.keys.len(), 8);
            assert!(installer.context.hws.is_empty());
            assert!(installer.context.connect_vault_members.is_empty());
            configure_native_node(&mut installer.context, chain);
            assert!(
                installer::proposal::reserve(&installer.context).is_err(),
                "MCP preparation cannot approve creation"
            );
            let mut review = Review::new(proposal.clone());
            review.load_context(&installer.context);
            assert!(review.apply(&mut installer.context));
            installer.context.wallet_alias = "changed after native review".into();
            assert!(installer::proposal::reserve(&installer.context).is_err());
            review.load_context(&installer.context);
            assert!(review.apply(&mut installer.context));
            let lease = installer::proposal::reserve(&installer.context).unwrap();
            assert!(!proposal.active());
            assert!(installer::proposal::reserve(&installer.context).is_err());
            denied(
                replacement
                    .call(
                        "tenshu_get_wallet_proposal",
                        json!({"proposal_id":current["proposal_id"]}),
                    )
                    .await,
            );
            assert!(
                !root.path().join(chain.dir_name()).join("data").exists(),
                "acceptance stops before wallet installation"
            );
            drop(lease);
            replacement.close().await;
            installer.stop();
            drop(installer);
            session::close();
        }
    });
}

#[test]
#[ignore = "build tenshu-mcp and set TENSHU_MCP_TEST_ADAPTER; run explicitly in platform CI"]
fn stdio_client_unsigned_transaction_reaches_native_review_on_every_chain() {
    use crate::app::{
        menu::{Menu, VaultSubMenu},
        message::Message as AppMessage,
        App,
    };
    use coincube_core::miniscript::bitcoin::{hashes::Hash, Address, ScriptBuf, WScriptHash};
    use iced::futures::StreamExt;
    let _guard = session::test_guard();
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        for network in wire::Chain::ALL {
            eprintln!("MCP acceptance workflow=transaction chain={}", network.as_str());
            let chain: ChainId = network.as_str().parse().unwrap();
            let (root, wallet, daemon) = super::tests::fixture_on_chain(chain);
            let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
            loop {
                let info = daemon.get_info().await.unwrap();
                if info.sync == 1.0 && info.last_poll_timestamp.is_some() { break; }
                assert!(tokio::time::Instant::now() < deadline, "synthetic wallet did not become ready");
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            let cube = CubeSettings::new_with_raw_id(CUBE.into(), "Disposable".into(), chain);
            let (mut app, startup) = App::new_inner(crate::app::Cache::default(), wallet.clone(), None, None,
                crate::app::Config::new(false), daemon.clone(), CoincubeDirectory::new(root.clone()), None, cube, None);
            drop(startup);
            session::open_without_pin(CUBE);
            *app.local_mcp = settings(&root, chain, true);
            let id = pair(&mut app.local_mcp, Some(wallet.clone()), Some(daemon.clone()));
            refresh(&mut app.local_mcp, &root, chain, Some(wallet.clone()), Some(daemon.clone()));
            let mut client = client(&mut app.local_mcp, &id).await;
            let listed = summary(client.call("tenshu_list_wallets", json!({})).await);
            assert_eq!(listed["wallets"][0]["network"], network.as_str());
            let wallet_id = listed["wallets"][0]["wallet_id"].clone();
            denied(client.call("tenshu_get_wallet_summary", json!({"wallet_id":wallet_id})).await);
            denied(client.call("tenshu_get_wallet_proposal", json!({"proposal_id":"foreign"})).await);
            let recipient = Address::from_script(&ScriptBuf::new_p2wsh(&WScriptHash::all_zeros()), chain.bitcoin_network()).unwrap().to_string();
            let input = json!({"network":network,"wallet_id":wallet_id,"request_id":"client-payment",
                "recipients":[{"address":recipient,"amount_sats":10000}],"fee_rate_sat_vb":2,"max_fee_sats":1000});
            for other in wire::Chain::ALL.iter().copied().filter(|other| *other != network) {
                let mut wrong = input.clone(); wrong["network"] = json!(other);
                denied(client.call("tenshu_prepare_transaction", wrong).await);
            }
            let mut missing = input.clone(); missing.as_object_mut().unwrap().remove("network");
            invalid_schema(&mut client, "tenshu_prepare_transaction", missing).await;
            let prepared = summary(client.call("tenshu_prepare_transaction", input.clone()).await);
            assert!(serde_json::from_value::<wire::transaction::Prepared>(prepared.clone()).is_ok(), "transaction summary contains unexpected fields");
            assert_eq!(prepared["network"], network.as_str());
            assert_eq!(prepared["status"], "unsigned_ready_for_native_review");
            assert_eq!(prepared["recipients"], input["recipients"]);
            assert_eq!(prepared["amount_sats"], 10000);
            assert_eq!(prepared["signing_required"], true);
            assert!(prepared["fee_sats"].as_u64().unwrap() <= 1000);
            assert!(prepared["fee_rate_sat_vb"].as_u64().unwrap() <= 2);
            assert_eq!(summary(client.call("tenshu_prepare_transaction", input.clone()).await), prepared);
            let lookup = json!({"wallet_id":wallet_id,"draft_id":prepared["draft_id"]});
            assert_eq!(summary(client.call("tenshu_get_prepared_transaction", lookup.clone()).await), prepared);
            let mut changed = input.clone(); changed["recipients"][0]["amount_sats"] = json!(10001);
            assert_eq!(client.call("tenshu_prepare_transaction", changed).await["structuredContent"]["error"]["code"], "transaction_conflict");
            denied(client.call("tenshu_get_prepared_transaction", json!({"wallet_id":wallet_id,"draft_id":"foreign"})).await);
            let service = app.local_mcp.transaction_service(&id, &wallet).unwrap();
            let native = service.native_checked(prepared["draft_id"].as_str().unwrap()).await.unwrap();
            drop(service);
            assert_eq!(native.chain, chain);
            assert!(native.psbt.inputs.iter().all(|i| i.partial_sigs.is_empty() && i.tap_key_sig.is_none() && i.tap_script_sigs.is_empty() && i.final_script_sig.is_none() && i.final_script_witness.is_none()));
            assert_eq!(coincube_core::spend::mcp_fee_cap(&native.psbt), Some((1000, 2)));
            let recipient_script = ScriptBuf::new_p2wsh(&WScriptHash::all_zeros());
            assert!(native.psbt.unsigned_tx.output.iter().any(|output| output.script_pubkey == recipient_script && output.value.to_sat() == 10000));
            let input_sats: u64 = native.coins.iter().map(|coin| coin.amount.to_sat()).sum();
            let output_sats: u64 = native.psbt.unsigned_tx.output.iter().map(|output| output.value.to_sat()).sum();
            assert_eq!(prepared["fee_sats"], input_sats - output_sats);
            assert_eq!(prepared["change_sats"], output_sats - 10000);
            // Execute the real native Review action and deliver its async result.
            let task = app.update(AppMessage::View(view::Message::LocalMcp(Message::ReviewTransaction {
                client_id: id.clone(), draft_id: prepared["draft_id"].as_str().unwrap().into(),
            })));
            let mut messages = iced_runtime::task::into_stream(task).expect("native review check");
            let mut delivered = false;
            while let Some(action) = messages.next().await {
                if let iced_runtime::Action::Output(message) = action {
                    assert!(matches!(&message, AppMessage::View(view::Message::LocalMcp(Message::ReviewTransactionChecked { native: Some(_), .. }))));
                    drop(app.update(message)); delivered = true;
                }
            }
            assert!(delivered);
            assert_eq!(app.panels.current, Menu::Vault(VaultSubMenu::Send));
            assert!(!app.panels.create_spend.as_ref().unwrap().keep_state());
            assert!(daemon.list_spend_txs().await.unwrap().spend_txs.is_empty());
            for tool in ["tenshu_sign_transaction", "tenshu_broadcast_transaction"] {
                assert_eq!(client.request("tools/call", json!({"name":tool,"arguments":{}})).await["error"]["code"], -32602);
            }
            session::close();
            denied(client.call("tenshu_get_prepared_transaction", lookup.clone()).await);
            assert!(app.local_mcp.transaction_service(&id, &wallet).is_none());
            app.local_mcp.revoke();
            session::open_without_pin(CUBE);
            refresh(&mut app.local_mcp, &root, chain, Some(wallet.clone()), Some(daemon.clone()));
            denied(client.call("tenshu_get_prepared_transaction", lookup).await);
            let fresh = summary(client.call("tenshu_prepare_transaction", input).await);
            assert_ne!(fresh["draft_id"], prepared["draft_id"]);
            revoke(&mut app.local_mcp, &id);
            unavailable(client.call("tenshu_get_prepared_transaction", json!({"wallet_id":wallet_id,"draft_id":fresh["draft_id"]})).await);
            client.close().await;
            assert!(daemon.list_spend_txs().await.unwrap().spend_txs.is_empty());
            tokio::task::block_in_place(|| app.stop()); drop(app);
            daemon.stop().await.unwrap(); drop(daemon); drop(wallet);
            session::close();
            std::fs::remove_dir_all(root).unwrap();
        }
    });
}

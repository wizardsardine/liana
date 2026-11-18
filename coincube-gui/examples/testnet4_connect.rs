//! Private stdin/stdout bridge for tests/e2e. Never registers devices or broadcasts.
//! Build with --features integration-tests --example testnet4_connect.

#[cfg(not(feature = "integration-tests"))]
fn main() {
    println!("{{\"ok\":false,\"code\":\"integration_feature_required\"}}");
    std::process::exit(2);
}

#[cfg(feature = "integration-tests")]
fn main() {
    // Libraries have no logging subscriber; only our fixed JSON protocol is emitted.
    // Do not let a panic accidentally print a descriptor or secret-bearing value.
    std::panic::set_hook(Box::new(|_| {}));
    let result = std::panic::catch_unwind(run::main).unwrap_or(Err("native_panic"));
    match result {
        Ok(value) => println!("{}", value),
        Err(code) => {
            println!("{}", serde_json::json!({"ok":false,"code":code}));
            std::process::exit(2);
        }
    }
}

#[cfg(feature = "integration-tests")]
mod run {
    use std::{
        collections::BTreeSet,
        fs::{self, OpenOptions},
        io::{Read, Write},
        path::{Path, PathBuf},
        str::FromStr,
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    use coincube_core::{
        descriptors::{CoincubeDescriptor, PathInfo},
        miniscript::{
            bitcoin::{bip32::Fingerprint, psbt::Psbt, secp256k1, Network},
            psbt::PsbtExt,
        },
        psbt_unified::UnifiedPsbt,
    };
    use coincube_gui::{
        app::{
            state::vault::psbt::merge_testnet4_integration_signatures,
            wallet::descriptor_id_fingerprint,
        },
        dir::CoincubeDirectory,
        services::{
            coincube::{CoincubeClient, CoincubeError},
            connect::{
                client::cache::ConnectCache,
                crypto::{seal_to_device, DeviceTransportKey},
                grpc::{
                    self, connect_v1::*, interceptor::AuthInterceptor, session::GrpcSessionClient,
                },
            },
        },
    };
    use serde::Deserialize;
    use serde_json::{json, Value};
    use sha2::{Digest, Sha256};

    type Result<T> = std::result::Result<T, &'static str>;
    const MAX_FILE: u64 = 4_000_000;
    const GRPC: &str = "https://grpc.coincube.io:443";

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Input {
        action: String,
        network: String,
        profile_dir: PathBuf,
        psbt_file: PathBuf,
        descriptor_file: PathBuf,
        expected_txid: String,
        cube_id: u64,
        vault_id: String,
        target_key_id: String,
        target_fingerprint: String,
        request_id: String,
        session_id: Option<String>,
        output_psbt_file: Option<PathBuf>,
        authorize_abort: Option<String>,
        abort_binding: Option<AbortBinding>,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct AbortBinding {
        owner_user_id: u32,
        creator_identity: String,
        psbt_sha256: String,
    }

    fn check(condition: bool, code: &'static str) -> Result<()> {
        if condition {
            Ok(())
        } else {
            Err(code)
        }
    }

    fn hex_string(value: &str, length: usize) -> bool {
        value.len() == length
            && value
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    }

    fn valid_request_id(request_id: &str) -> bool {
        request_id.len() == 36
            && request_id.bytes().enumerate().all(|(i, b)| {
                if [8, 13, 18, 23].contains(&i) {
                    b == b'-'
                } else {
                    b.is_ascii_hexdigit() && !b.is_ascii_uppercase()
                }
            })
    }

    fn validate_input(input: &Input) -> Result<()> {
        check(input.network == "testnet4", "wrong_network")?;
        check(
            matches!(
                input.action.as_str(),
                "inspect" | "request" | "collect" | "cancel" | "cleanup_audit"
            ),
            "invalid_action",
        )?;
        check(
            hex_string(&input.expected_txid, 64) && hex_string(&input.target_fingerprint, 8),
            "invalid_identity",
        )?;
        check(
            input.cube_id > 0
                && input.vault_id.parse::<u64>().unwrap_or(0) > 0
                && input.target_key_id.parse::<u64>().unwrap_or(0) > 0,
            "invalid_identity",
        )?;
        check(valid_request_id(&input.request_id), "invalid_request_id")?;
        for path in [&input.profile_dir, &input.psbt_file, &input.descriptor_file] {
            check(path.is_absolute(), "absolute_paths_required")?;
        }
        if matches!(input.action.as_str(), "collect" | "cancel") {
            check(
                input
                    .session_id
                    .as_ref()
                    .is_some_and(|id| !id.is_empty() && id.len() < 128),
                "session_id_required",
            )?;
        }
        if input.action == "cancel" {
            check(
                input
                    .authorize_abort
                    .as_ref()
                    .is_some_and(|v| !v.trim().is_empty() && v.len() <= 512),
                "abort_authorization_required",
            )?;
            let binding = input
                .abort_binding
                .as_ref()
                .ok_or("abort_binding_required")?;
            check(
                binding.owner_user_id > 0
                    && hex_string(&binding.creator_identity, 64)
                    && hex_string(&binding.psbt_sha256, 64),
                "invalid_abort_binding",
            )?;
        } else {
            check(
                input.authorize_abort.is_none() && input.abort_binding.is_none(),
                "unexpected_abort_authorization",
            )?;
        }
        if input.action == "collect" {
            check(
                input.output_psbt_file.as_ref().is_some_and(|p| {
                    p.is_absolute() && p != &input.psbt_file && p != &input.descriptor_file
                }),
                "output_file_required",
            )?;
        }
        Ok(())
    }

    fn private_read(path: &Path) -> Result<String> {
        let metadata = fs::symlink_metadata(path).map_err(|_| "fixture_file_unavailable")?;
        check(
            metadata.is_file() && !metadata.file_type().is_symlink() && metadata.len() <= MAX_FILE,
            "invalid_fixture_file",
        )?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            check(
                metadata.permissions().mode() & 0o077 == 0,
                "private_file_permissions_required",
            )?;
        }
        fs::read_to_string(path).map_err(|_| "fixture_file_unavailable")
    }

    fn atomic_private_write(path: &Path, text: &str) -> Result<()> {
        let parent = path.parent().ok_or("output_file_required")?;
        check(parent.is_dir(), "output_directory_unavailable")?;
        if let Ok(metadata) = fs::symlink_metadata(path) {
            check(
                metadata.is_file() && !metadata.file_type().is_symlink(),
                "invalid_output_file",
            )?;
        }
        let tmp = parent.join(format!(
            ".testnet4-native-{}-{:016x}",
            std::process::id(),
            rand::random::<u64>()
        ));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let outcome = (|| {
            let mut file = options.open(&tmp).map_err(|_| "output_write_failed")?;
            file.write_all(text.as_bytes())
                .map_err(|_| "output_write_failed")?;
            file.sync_all().map_err(|_| "output_write_failed")?;
            fs::rename(&tmp, path).map_err(|_| "output_write_failed")?;
            fs::File::open(parent)
                .and_then(|d| d.sync_all())
                .map_err(|_| "output_write_failed")?;
            Ok(())
        })();
        if outcome.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        outcome
    }

    /// Verify the prepared transaction against authenticated prevouts and descriptor
    /// derivations before any remote mutation. Only native P2WSH primary spends.
    fn fixture(input: &Input) -> Result<(CoincubeDescriptor, Psbt, Fingerprint)> {
        let descriptor = CoincubeDescriptor::from_str(private_read(&input.descriptor_file)?.trim())
            .map_err(|_| "invalid_descriptor")?;
        let psbt =
            Psbt::from_str(private_read(&input.psbt_file)?.trim()).map_err(|_| "invalid_psbt")?;
        let fingerprint =
            Fingerprint::from_str(&input.target_fingerprint).map_err(|_| "invalid_identity")?;
        check(
            descriptor.all_xpubs_net_is(Network::Testnet4),
            "wrong_descriptor_network",
        )?;
        check(
            matches!(descriptor.policy().primary_path(), PathInfo::Multi(2, keys) if keys.len() == 3),
            "expected_two_of_three",
        )?;
        check(
            descriptor.contains_fingerprint_in_path(fingerprint, None),
            "target_not_in_primary_policy",
        )?;
        check(
            psbt.unsigned_tx.compute_txid().to_string() == input.expected_txid,
            "transaction_intent_mismatch",
        )?;
        check(
            !psbt.inputs.is_empty()
                && psbt.inputs.len() <= 100
                && psbt.inputs.len() == psbt.unsigned_tx.input.len()
                && psbt.outputs.len() == psbt.unsigned_tx.output.len(),
            "invalid_psbt",
        )?;
        let secp = secp256k1::Secp256k1::verification_only();
        for (txin, pin) in psbt.unsigned_tx.input.iter().zip(&psbt.inputs) {
            check(
                txin.sequence.to_consensus_u32() & 0x8000_0000 != 0,
                "primary_path_required",
            )?;
            let previous = pin
                .non_witness_utxo
                .as_ref()
                .ok_or("authenticated_prevout_required")?;
            check(
                previous.compute_txid() == txin.previous_output.txid,
                "prevout_mismatch",
            )?;
            let prevout = previous
                .output
                .get(txin.previous_output.vout as usize)
                .ok_or("prevout_mismatch")?;
            check(
                pin.witness_utxo.as_ref() == Some(prevout),
                "prevout_mismatch",
            )?;
            let script = pin.witness_script.as_ref().ok_or("p2wsh_required")?;
            check(
                script.to_p2wsh() == prevout.script_pubkey,
                "prevout_mismatch",
            )?;
            let indexes: BTreeSet<_> = pin
                .bip32_derivation
                .values()
                .filter_map(|(_, path)| path.as_ref().last().copied())
                .filter(|n| n.is_normal())
                .collect();
            check(indexes.len() <= 12, "invalid_derivations")?;
            let mut matched = false;
            for index in indexes {
                for branch in [
                    descriptor.receive_descriptor(),
                    descriptor.change_descriptor(),
                ] {
                    let derived = branch.derive(index, &secp);
                    if derived.script_pubkey() == prevout.script_pubkey {
                        let mut expected = Default::default();
                        derived.update_psbt_in(&mut expected);
                        check(
                            expected.bip32_derivation == pin.bip32_derivation
                                && expected.witness_script == pin.witness_script,
                            "descriptor_origin_mismatch",
                        )?;
                        matched = true;
                    }
                }
            }
            check(matched, "input_not_in_descriptor")?;
        }
        let unified = UnifiedPsbt::from_psbt(psbt.clone()).map_err(|_| "invalid_psbt")?;
        coincube_core::unified_finalize::verify_all_signatures(&unified, &secp)
            .map_err(|_| "invalid_existing_signature")?;
        let progress = descriptor
            .partial_spend_info(&psbt)
            .map_err(|_| "inconsistent_signature_progress")?;
        check(
            progress.primary_path().sigs_count == 1
                && !progress
                    .primary_path()
                    .signed_pubkeys
                    .contains_key(&fingerprint),
            "expected_one_local_signature",
        )?;
        Ok((descriptor, psbt, fingerprint))
    }

    fn rpc_error(error: tonic::Status) -> &'static str {
        match error.code() {
            tonic::Code::Unauthenticated => "connect_auth_required",
            tonic::Code::PermissionDenied => "connect_permission_denied",
            tonic::Code::Unavailable | tonic::Code::DeadlineExceeded => "connect_unavailable",
            _ => "connect_rpc_refused",
        }
    }

    fn rest_error(error: CoincubeError) -> &'static str {
        match error {
            CoincubeError::Unsuccessful(info) => match info.status_code {
                401 => "connect_auth_required",
                403 => "connect_permission_denied",
                404 => "connect_fixture_not_found",
                429 | 500..=599 => "connect_unavailable",
                _ => "connect_rest_refused",
            },
            CoincubeError::Network(_) | CoincubeError::RateLimited { .. } => "connect_unavailable",
            _ => "connect_response_invalid",
        }
    }

    fn validate_session(
        input: &Input,
        session: &SigningSession,
        descriptor_id: &str,
        device: &str,
        key: &DeviceTransportKey,
        target: &SignerTarget,
    ) -> Result<()> {
        check(
            session.network == "testnet4"
                && session.vault_id == input.vault_id
                && session.request_id == input.request_id
                && session.descriptor_id == descriptor_id
                && session.created_by_device_id == device
                && session.note == format!("Testnet4 QA {}", input.request_id),
            "session_binding_mismatch",
        )?;
        if matches!(input.action.as_str(), "collect" | "cancel") {
            check(
                input.session_id.as_deref() == Some(session.session_id.as_str()),
                "session_binding_mismatch",
            )?;
        }
        check(
            session.payload_scheme == PayloadScheme::EciesV1 as i32
                && session.psbt.is_empty()
                && session.creator_transport_pubkey == key.public_key()
                && !session.is_recovery_spend,
            "session_encryption_mismatch",
        )?;
        check(
            session.targets.len() == 1
                && session.targets[0].key_id == input.target_key_id
                && session.targets[0].key_fingerprint == input.target_fingerprint
                && session.targets[0].device_id == target.device_id
                // sessionToProto intentionally omits target transport keys;
                // ResolveSigners supplies the key used for the request envelope.
                && (session.targets[0].transport_pubkey.is_empty()
                    || session.targets[0].transport_pubkey == target.transport_pubkey),
            "session_target_mismatch",
        )
    }

    // Cancel only unsigned active requests, or reconcile our exact already-
    // cancelled request. Never adopt rejected/expired/failed or signed states.
    fn cancellation_state(session: &SigningSession, terminal: bool) -> Result<bool> {
        check(
            session.submitted_signatures.is_empty(),
            "abort_signature_present",
        )?;
        if session.status == SessionStatus::Cancelled as i32 {
            return Ok(false);
        }
        check(
            !terminal && (1..=4).contains(&session.status),
            "abort_not_unsigned_or_cancelled",
        )?;
        Ok(true)
    }

    fn collection_progress(
        input: &Input,
        session: &SigningSession,
        descriptor_id: &str,
        device: &str,
        key: &DeviceTransportKey,
        target: &SignerTarget,
    ) -> Result<Value> {
        validate_session(input, session, descriptor_id, device, key, target)?;
        check(
            (1..=6).contains(&session.status),
            "session_terminal_or_unknown",
        )?;
        Ok(json!({"status":session.status,
            "submitted_signature_count":session.submitted_signatures.len()}))
    }

    /// Validate and merge the encrypted signatures returned for this exact request.
    fn merge_returned_signatures(
        input: &Input,
        session: &SigningSession,
        device: &str,
        key: &DeviceTransportKey,
        psbt: &Psbt,
        fingerprint: Fingerprint,
    ) -> Result<Psbt> {
        let mut merged = psbt.clone();
        for signature in &session.submitted_signatures {
            check(
                signature.signed_psbt.is_empty()
                    && signature.device_id == session.targets[0].device_id
                    && signature
                        .signed_key_ids
                        .iter()
                        .any(|id| id == &input.target_key_id),
                "signature_target_mismatch",
            )?;
            let envelope = signature
                .signature_envelope
                .as_ref()
                .ok_or("unencrypted_signature_refused")?;
            check(
                envelope.device_id.is_empty() || envelope.device_id == device,
                "signature_target_mismatch",
            )?;
            let bytes = key
                .open(
                    &envelope.ephemeral_pubkey,
                    &envelope.nonce,
                    &envelope.ciphertext,
                    &input.request_id,
                )
                .map_err(|_| "signature_open_failed")?;
            let delta = Psbt::deserialize(&bytes).map_err(|_| "invalid_returned_psbt")?;
            check(
                delta.unsigned_tx == psbt.unsigned_tx && delta.inputs.len() == psbt.inputs.len(),
                "returned_transaction_mismatch",
            )?;
            for (local, remote) in psbt.inputs.iter().zip(&delta.inputs) {
                check(
                    remote.tap_script_sigs.is_empty() && remote.tap_key_sig.is_none(),
                    "unexpected_signature_kind",
                )?;
                for (pubkey, sig) in &remote.partial_sigs {
                    if let Some(existing) = local.partial_sigs.get(pubkey) {
                        check(existing == sig, "signature_conflict")?;
                    } else {
                        check(
                            local
                                .bip32_derivation
                                .get(&pubkey.inner)
                                .is_some_and(|(fp, _)| fp == &fingerprint),
                            "unexpected_signing_key",
                        )?;
                    }
                }
            }
            merge_testnet4_integration_signatures(&mut merged, &delta)
                .map_err(|_| "signature_merge_failed")?;
        }
        Ok(merged)
    }

    fn cleanup_inventory(
        input: &Input,
        owner: u32,
        pending: &ListPendingSessionsResponse,
        decrypt: &ListPendingDecryptRequestsResponse,
    ) -> Value {
        json!({"ok":true,"network":"testnet4","txid":input.expected_txid,
            "owner_user_id":owner,
            "sessions":pending.sessions.iter().map(|s| json!({"vault_id":s.vault_id,
                "key_ids":s.targets.iter().map(|t| t.key_id.clone()).collect::<Vec<_>>()})).collect::<Vec<_>>(),
            "decrypt_cubes":decrypt.requests.iter().map(|r| r.cube_id.clone()).collect::<Vec<_>>() })
    }

    async fn execute(input: Input) -> Result<Value> {
        validate_input(&input)?;
        let (descriptor, psbt, fingerprint) = fixture(&input)?;
        let descriptor_id = descriptor_id_fingerprint(&descriptor).to_string();
        let network_dir =
            CoincubeDirectory::new(input.profile_dir.clone()).network_directory(Network::Testnet4);
        // Validate private file mode and refuse replacement of missing/corrupt keys.
        let _ = private_read(&network_dir.path().join("connect_transport_key"))?;
        let _ = private_read(&network_dir.path().join("connect.json"))?;
        let key = DeviceTransportKey::load_existing_for_integration(&network_dir)
            .map_err(|_| "transport_key_unavailable")?;
        let cache = ConnectCache::from_file(&network_dir).map_err(|_| "connect_auth_required")?;
        let account = cache.active_account().ok_or("connect_auth_required")?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| "clock_error")?
            .as_secs() as i64;
        check(
            account.tokens.expires_at > now + 30 && !account.tokens.access_token.is_empty(),
            "connect_auth_expired",
        )?;
        let device = account
            .device_id
            .as_ref()
            .filter(|s| !s.is_empty())
            .ok_or("connect_device_required")?;
        // Set before constructing the REST client; never load a project .env.
        std::env::set_var("COINCUBE_API_URL", "https://api.coincube.io");
        let mut rest = CoincubeClient::new();
        rest.set_token(&account.tokens.access_token);
        if let Some(binding) = &input.abort_binding {
            let user = rest.get_user().await.map_err(rest_error)?;
            check(user.id == binding.owner_user_id, "abort_account_mismatch")?;
            let creator = serde_json::to_vec(device).map_err(|_| "abort_creator_mismatch")?;
            check(
                hex::encode(Sha256::digest(&creator)) == binding.creator_identity,
                "abort_creator_mismatch",
            )?;
            check(
                hex::encode(Sha256::digest(private_read(&input.psbt_file)?)) == binding.psbt_sha256,
                "abort_psbt_mismatch",
            )?;
        }
        if input.action == "cleanup_audit" {
            let owner = rest.get_user().await.map_err(rest_error)?;
            let channel = grpc::create_channel(GRPC)
                .await
                .map_err(|_| "connect_unavailable")?;
            let mut client = GrpcSessionClient::new(
                channel,
                AuthInterceptor::with_device_id(&account.tokens.access_token, device.clone()),
            );
            let pending = client.list_pending_sessions().await.map_err(rpc_error)?;
            let decrypt = client
                .list_pending_decrypt_requests()
                .await
                .map_err(rpc_error)?;
            return Ok(cleanup_inventory(&input, owner.id, &pending, &decrypt));
        }
        let cube = rest.get_cube(input.cube_id).await.map_err(rest_error)?;
        check(cube.network == "testnet4", "connect_cube_wrong_network")?;
        let vault = rest
            .get_connect_vault(input.cube_id)
            .await
            .map_err(rest_error)?;
        check(
            vault.id.to_string() == input.vault_id
                && vault.cube_id == input.cube_id
                && vault.fingerprint == descriptor_id,
            "connect_vault_mismatch",
        )?;
        let channel = grpc::create_channel(GRPC)
            .await
            .map_err(|_| "connect_unavailable")?;
        let mut client = GrpcSessionClient::new(
            channel,
            AuthInterceptor::with_device_id(&account.tokens.access_token, device.clone()),
        );
        let mut output = json!({"ok":true,"action":input.action,"network":"testnet4","txid":input.expected_txid,"descriptor_id":descriptor_id,"signature_count_min":1,"ready":false,"fee_sats":psbt.fee().map_err(|_| "invalid_fee")?.to_sat()});
        let resolved = client
            .resolve_signers(input.vault_id.clone())
            .await
            .map_err(rpc_error)?;
        let matches: Vec<_> = resolved
            .targets
            .into_iter()
            .filter(|t| {
                t.key_id == input.target_key_id && t.key_fingerprint == input.target_fingerprint
            })
            .collect();
        check(matches.len() == 1, "target_unresolved")?;
        let target = matches.into_iter().next().ok_or("target_unresolved")?;
        check(
            !target.device_id.is_empty() && target.transport_pubkey.len() == 33,
            "target_transport_unavailable",
        )?;
        if input.action == "inspect" || input.action == "request" {
            if input.action == "inspect" {
                return Ok(output);
            }
            let wire = descriptor
                .prune_bip32_derivs_last_avail(psbt)
                .map_err(|_| "invalid_spending_path")?
                .serialize();
            let seal = |bytes: &[u8]| -> Result<PayloadEnvelope> {
                let sealed = seal_to_device(&target.transport_pubkey, &input.request_id, bytes)
                    .map_err(|_| "payload_seal_failed")?;
                Ok(PayloadEnvelope {
                    device_id: target.device_id.clone(),
                    ephemeral_pubkey: sealed.ephemeral_pubkey,
                    nonce: sealed.nonce,
                    ciphertext: sealed.ciphertext,
                })
            };
            let request = CreateSigningSessionRequest {
                request_id: input.request_id.clone(),
                vault_id: input.vault_id.clone(),
                descriptor_id: descriptor_id.clone(),
                psbt_envelopes: vec![seal(&wire)?],
                descriptor_envelopes: vec![seal(descriptor.to_string().as_bytes())?],
                targets: vec![target.clone()],
                payload_scheme: PayloadScheme::EciesV1 as i32,
                note: format!("Testnet4 QA {}", input.request_id),
                network: "testnet4".into(),
                ttl: Some(prost_types::Duration {
                    seconds: 86400,
                    nanos: 0,
                }),
                ..Default::default()
            };
            let session = client
                .create_signing_session(request)
                .await
                .map_err(rpc_error)?;
            validate_session(&input, &session, &descriptor_id, device, &key, &target)?;
            check(
                (1..=6).contains(&session.status),
                "session_terminal_or_unknown",
            )?;
            output["session_id"] = json!(session.session_id);
            output["request_id"] = json!(input.request_id);
            output["status"] = json!(session.status);
            return Ok(output);
        }
        let session = client
            .get_signing_session(input.session_id.clone().ok_or("session_id_required")?)
            .await
            .map_err(rpc_error)?
            .session
            .ok_or("session_missing")?;
        if input.action == "cancel" {
            validate_session(&input, &session, &descriptor_id, device, &key, &target)?;
            let needs_cancel = cancellation_state(&session, false)?;
            if needs_cancel {
                client
                    .cancel_signing_session(
                        session.session_id.clone(),
                        "qa_interrupted_pin_attempt".into(),
                    )
                    .await
                    .map_err(rpc_error)?;
            }
            // A successful mutation response is not terminal evidence. A raced
            // signature or changed binding must never release a credential.
            let terminal = client
                .get_signing_session(session.session_id.clone())
                .await
                .map_err(rpc_error)?
                .session
                .ok_or("session_missing")?;
            validate_session(&input, &terminal, &descriptor_id, device, &key, &target)?;
            cancellation_state(&terminal, true)?;
            output["cancelled"] = json!(true);
            output["owner_user_id"] = json!(
                input
                    .abort_binding
                    .as_ref()
                    .ok_or("abort_binding_required")?
                    .owner_user_id
            );
            output["session_id"] = json!(terminal.session_id);
            output["request_id"] = json!(input.request_id);
            output["status"] = json!(terminal.status);
            output["submitted_signature_count"] = json!(0);
            output["descriptor_id"] = json!(descriptor_id);
            output["creator_device_id"] = json!(device);
            output["target_device_id"] = json!(target.device_id);
            output["target_key_id"] = json!(input.target_key_id);
            output["target_fingerprint"] = json!(input.target_fingerprint);
            return Ok(output);
        }
        let progress =
            collection_progress(&input, &session, &descriptor_id, device, &key, &target)?;
        output["status"] = progress["status"].clone();
        output["submitted_signature_count"] = progress["submitted_signature_count"].clone();
        if session.submitted_signatures.is_empty() {
            check(
                session.status != SessionStatus::Completed as i32,
                "completed_without_signature",
            )?;
            return Ok(output);
        }
        let merged = merge_returned_signatures(&input, &session, device, &key, &psbt, fingerprint)?;
        let secp = secp256k1::Secp256k1::verification_only();
        let mut finalized = merged.clone();
        finalized
            .finalize_mut(&secp)
            .map_err(|_| "signatures_incomplete_or_invalid")?;
        finalized
            .extract(&secp)
            .map_err(|_| "signature_verification_failed")?;
        let progress = descriptor
            .partial_spend_info(&merged)
            .map_err(|_| "inconsistent_signature_progress")?;
        check(
            progress.primary_path().sigs_count >= 2
                && progress
                    .primary_path()
                    .signed_pubkeys
                    .contains_key(&fingerprint),
            "phone_signature_missing",
        )?;
        atomic_private_write(
            input
                .output_psbt_file
                .as_ref()
                .ok_or("output_file_required")?,
            &merged.to_string(),
        )?;
        output["signature_count_min"] = json!(progress.primary_path().sigs_count);
        output["ready"] = json!(true);
        Ok(output)
    }

    pub fn main() -> Result<Value> {
        let mut data = String::new();
        std::io::stdin()
            .take(32_769)
            .read_to_string(&mut data)
            .map_err(|_| "invalid_input")?;
        check(data.len() <= 32_768, "input_too_large")?;
        let input: Input = serde_json::from_str(&data).map_err(|_| "invalid_input")?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| "runtime_unavailable")?;
        runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(45), execute(input))
                .await
                .map_err(|_| "connect_timeout")?
        })
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        struct TempDir(PathBuf);
        impl TempDir {
            fn new() -> Self {
                let path = std::env::temp_dir().join(format!(
                    "testnet4-native-test-{:016x}",
                    rand::random::<u64>()
                ));
                fs::create_dir(&path).unwrap();
                Self(path)
            }
        }
        impl Drop for TempDir {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }

        fn input() -> Input {
            serde_json::from_value(json!({"action":"inspect","network":"testnet4","profile_dir":"/private/fixture","psbt_file":"/private/draft.psbt","descriptor_file":"/private/descriptor","expected_txid":"ab".repeat(32),"cube_id":1,"vault_id":"1","target_key_id":"2","target_fingerprint":"12345678","request_id":"12345678-1234-4234-8234-123456789abc"})).unwrap()
        }

        #[test]
        fn cleanup_inventory_retains_all_active_references_without_payloads() {
            let mut request = input();
            request.action = "cleanup_audit".into();
            assert!(validate_input(&request).is_ok());
            let sessions = ListPendingSessionsResponse {
                sessions: vec![SigningSession {
                    vault_id: "42".into(),
                    psbt: b"private opaque payload".to_vec(),
                    targets: vec![SignerTarget {
                        key_id: "7".into(),
                        device_id: "private-device".into(),
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
            };
            let decrypt = ListPendingDecryptRequestsResponse {
                requests: vec![DecryptRequestedEvent {
                    cube_id: "99".into(),
                    ephemeral_pubkey: b"opaque key".to_vec(),
                    ..Default::default()
                }],
            };
            let value = cleanup_inventory(&request, 5, &sessions, &decrypt);
            assert_eq!(
                value["sessions"],
                json!([{"vault_id":"42", "key_ids":["7"]}])
            );
            assert_eq!(value["decrypt_cubes"], json!(["99"]));
            assert_eq!(value["owner_user_id"], 5);
            assert!(!value.to_string().contains("private"));
            assert!(!value.to_string().contains("opaque"));
            assert!(value.get("psbt").is_none());
        }

        #[test]
        fn refuses_other_chains_and_invalid_checkpoint_identity_before_io() {
            let mut fixture = input();
            assert!(validate_input(&fixture).is_ok());
            fixture.network = "bitcoin".into();
            assert_eq!(validate_input(&fixture), Err("wrong_network"));
            fixture.network = "testnet4".into();
            fixture.request_id = "new-every-run".into();
            assert_eq!(validate_input(&fixture), Err("invalid_request_id"));
        }

        #[test]
        fn collection_requires_exact_session_and_separate_output() {
            let mut fixture = input();
            fixture.action = "collect".into();
            assert_eq!(validate_input(&fixture), Err("session_id_required"));
            fixture.session_id = Some("test-session".into());
            fixture.output_psbt_file = Some(fixture.psbt_file.clone());
            assert_eq!(validate_input(&fixture), Err("output_file_required"));
            fixture.output_psbt_file = Some(PathBuf::from("/private/signed.psbt"));
            assert!(validate_input(&fixture).is_ok());
        }

        #[test]
        fn cancellation_requires_authorization_and_exact_session() {
            let mut fixture = input();
            fixture.action = "cancel".into();
            assert_eq!(validate_input(&fixture), Err("session_id_required"));
            fixture.session_id = Some("session".into());
            assert_eq!(
                validate_input(&fixture),
                Err("abort_authorization_required")
            );
            fixture.authorize_abort = Some("owner QA instruction".into());
            assert_eq!(validate_input(&fixture), Err("abort_binding_required"));
            fixture.abort_binding = Some(AbortBinding {
                owner_user_id: 1,
                creator_identity: "ab".repeat(32),
                psbt_sha256: "cd".repeat(32),
            });
            assert!(validate_input(&fixture).is_ok());
            fixture.action = "inspect".into();
            assert_eq!(
                validate_input(&fixture),
                Err("unexpected_abort_authorization")
            );
        }

        #[test]
        fn cancellation_never_accepts_signature_races_or_other_terminal_states() {
            for status in 0..=11 {
                let mut session = SigningSession {
                    status,
                    ..Default::default()
                };
                assert_eq!(
                    cancellation_state(&session, false).is_ok(),
                    (1..=4).contains(&status) || status == 8
                );
                assert_eq!(cancellation_state(&session, true).is_ok(), status == 8);
                session.submitted_signatures.push(Default::default());
                assert_eq!(
                    cancellation_state(&session, false),
                    Err("abort_signature_present")
                );
                assert_eq!(
                    cancellation_state(&session, true),
                    Err("abort_signature_present")
                );
            }
        }

        #[test]
        fn rpc_errors_never_expose_server_messages() {
            assert_eq!(
                rpc_error(tonic::Status::unauthenticated("secret-test-token")),
                "connect_auth_required"
            );
            assert_eq!(
                rpc_error(tonic::Status::internal("private wallet payload")),
                "connect_rpc_refused"
            );
        }

        #[test]
        fn existing_transport_loader_never_mints_or_replaces_a_key() {
            let dir = TempDir::new();
            let network = coincube_gui::dir::NetworkDirectory::new(dir.0.clone());
            let path = dir.0.join("connect_transport_key");
            assert!(DeviceTransportKey::load_existing_for_integration(&network).is_err());
            assert!(!path.exists());
            fs::write(&path, "corrupt").unwrap();
            assert!(DeviceTransportKey::load_existing_for_integration(&network).is_err());
            assert_eq!(fs::read_to_string(&path).unwrap(), "corrupt");
        }

        #[test]
        fn session_must_match_checkpoint_chain_device_note_and_target() {
            let request = input();
            let dir = TempDir::new();
            let network = coincube_gui::dir::NetworkDirectory::new(dir.0.clone());
            let key = DeviceTransportKey::load_or_create(&network).unwrap();
            let target = SignerTarget {
                device_id: "phone-fixture".into(),
                key_id: request.target_key_id.clone(),
                key_fingerprint: request.target_fingerprint.clone(),
                transport_pubkey: key.public_key().to_vec(),
                ..Default::default()
            };
            let mut session = SigningSession {
                session_id: "session-fixture".into(),
                request_id: request.request_id.clone(),
                vault_id: request.vault_id.clone(),
                descriptor_id: "12345678".into(),
                created_by_device_id: "desktop-fixture".into(),
                note: format!("Testnet4 QA {}", request.request_id),
                network: "testnet4".into(),
                creator_transport_pubkey: key.public_key().to_vec(),
                targets: vec![target.clone()],
                payload_scheme: PayloadScheme::EciesV1 as i32,
                ..Default::default()
            };
            // The API deliberately omits target keys from session responses.
            session.targets[0].transport_pubkey.clear();
            assert!(validate_session(
                &request,
                &session,
                "12345678",
                "desktop-fixture",
                &key,
                &target
            )
            .is_ok());
            let mut collect = input();
            collect.action = "collect".into();
            collect.session_id = Some(session.session_id.clone());
            session.status = SessionStatus::Viewed as i32;
            let progress = collection_progress(
                &collect,
                &session,
                "12345678",
                "desktop-fixture",
                &key,
                &target,
            )
            .unwrap();
            assert_eq!(progress["submitted_signature_count"], 0);
            assert_eq!(progress["status"], SessionStatus::Viewed as i32);
            session.submitted_signatures.push(Default::default());
            let progress = collection_progress(
                &collect,
                &session,
                "12345678",
                "desktop-fixture",
                &key,
                &target,
            )
            .unwrap();
            assert_eq!(progress["submitted_signature_count"], 1);
            collect.session_id = Some("other-session".into());
            assert_eq!(
                collection_progress(
                    &collect,
                    &session,
                    "12345678",
                    "desktop-fixture",
                    &key,
                    &target
                ),
                Err("session_binding_mismatch")
            );
            collect.session_id = Some(session.session_id.clone());
            session.targets[0].key_id = "other-key".into();
            assert_eq!(
                collection_progress(
                    &collect,
                    &session,
                    "12345678",
                    "desktop-fixture",
                    &key,
                    &target
                ),
                Err("session_target_mismatch")
            );
            session.targets[0] = target.clone();
            session.network = "bitcoin".into();
            assert_eq!(
                validate_session(
                    &request,
                    &session,
                    "12345678",
                    "desktop-fixture",
                    &key,
                    &target
                ),
                Err("session_binding_mismatch")
            );
            session.network = "testnet4".into();
            session.targets[0].device_id = "other-phone".into();
            assert_eq!(
                validate_session(
                    &request,
                    &session,
                    "12345678",
                    "desktop-fixture",
                    &key,
                    &target
                ),
                Err("session_target_mismatch")
            );
            session.targets[0] = target.clone();
            session.note.clear();
            assert_eq!(
                validate_session(
                    &request,
                    &session,
                    "12345678",
                    "desktop-fixture",
                    &key,
                    &target
                ),
                Err("session_binding_mismatch")
            );
        }

        #[test]
        fn valid_prepared_fixture_is_accepted_but_forged_prevout_is_refused() {
            use coincube_core::miniscript::{
                bitcoin::{
                    self, Amount, OutPoint, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Witness,
                },
                descriptor::DescriptorPublicKey,
            };
            use coincube_core::{descriptors::CoincubePolicy, signer::MasterSigner};
            let secp = secp256k1::Secp256k1::new();
            let signers: Vec<_> = (0..3)
                .map(|_| MasterSigner::generate(Network::Testnet4).unwrap())
                .collect();
            let key = |i: usize, branches: &str| {
                DescriptorPublicKey::from_str(&format!(
                    "[{}]{}/<{}>/*",
                    signers[i].fingerprint(&secp),
                    signers[i].xpub_at(&bitcoin::bip32::DerivationPath::master(), &secp),
                    branches
                ))
                .unwrap()
            };
            let descriptor = CoincubeDescriptor::new(
                CoincubePolicy::new_legacy(
                    PathInfo::Multi(2, vec![key(0, "0;1"), key(1, "0;1"), key(2, "0;1")]),
                    vec![(65535, PathInfo::Single(key(2, "2;3")))]
                        .into_iter()
                        .collect(),
                )
                .unwrap(),
            );
            let derived = descriptor.receive_descriptor().derive(
                bitcoin::bip32::ChildNumber::from_normal_idx(0).unwrap(),
                &secp,
            );
            let previous = Transaction {
                version: bitcoin::transaction::Version::TWO,
                lock_time: bitcoin::absolute::LockTime::ZERO,
                input: vec![TxIn {
                    previous_output: OutPoint::null(),
                    script_sig: ScriptBuf::new(),
                    sequence: Sequence::MAX,
                    witness: Witness::default(),
                }],
                output: vec![TxOut {
                    value: Amount::from_sat(20_000),
                    script_pubkey: derived.script_pubkey(),
                }],
            };
            let spending = Transaction {
                version: bitcoin::transaction::Version::TWO,
                lock_time: bitcoin::absolute::LockTime::ZERO,
                input: vec![TxIn {
                    previous_output: OutPoint::new(previous.compute_txid(), 0),
                    script_sig: ScriptBuf::new(),
                    sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
                    witness: Witness::default(),
                }],
                output: vec![TxOut {
                    value: Amount::from_sat(19_000),
                    script_pubkey: derived.script_pubkey(),
                }],
            };
            let mut psbt = Psbt::from_unsigned_tx(spending).unwrap();
            derived.update_psbt_in(&mut psbt.inputs[0]);
            psbt.inputs[0].witness_utxo = Some(previous.output[0].clone());
            psbt.inputs[0].non_witness_utxo = Some(previous);
            let signed = signers[0].sign_psbt(psbt.clone(), &secp).unwrap();
            let dir = TempDir::new();
            let mut request = input();
            request.expected_txid = signed.unsigned_tx.compute_txid().to_string();
            request.target_fingerprint = signers[1].fingerprint(&secp).to_string();
            request.psbt_file = dir.0.join("draft.psbt");
            request.descriptor_file = dir.0.join("descriptor");
            atomic_private_write(&request.psbt_file, &signed.to_string()).unwrap();
            atomic_private_write(&request.descriptor_file, &descriptor.to_string()).unwrap();
            assert!(fixture(&request).is_ok());
            atomic_private_write(&request.psbt_file, &psbt.to_string()).unwrap();
            assert!(matches!(
                fixture(&request),
                Err("expected_one_local_signature")
            ));
            let mut forged = signed;
            forged.inputs[0].witness_utxo.as_mut().unwrap().value = Amount::from_sat(30_000);
            atomic_private_write(&request.psbt_file, &forged.to_string()).unwrap();
            assert!(matches!(fixture(&request), Err("prevout_mismatch")));
        }
    }
}

//! Chain-bound unsigned constructions. Holds public policy and a daemon, never a signer.
use crate::{
    app::wallet::SyncStatus,
    chain::ChainId,
    daemon::{
        model::{Coin, CreateSpendResult},
        Daemon,
    },
};
use coincube_core::{
    descriptors::CoincubeDescriptor,
    miniscript::bitcoin::{secp256k1, Address, BlockHash, Psbt},
};
use coincube_mcp::{
    self as wire,
    transaction::{DraftInput, PrepareInput, Prepared},
};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    str::FromStr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

/// A native-only construction; never serializable over MCP.
#[derive(Clone)]
pub struct NativeDraft {
    pub chain: ChainId,
    pub psbt: Psbt,
    pub coins: Vec<Coin>,
    pub warnings: Vec<String>,
}
impl std::fmt::Debug for NativeDraft {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("NativeDraft(<redacted>)")
    }
}
struct Draft {
    input: PrepareInput,
    result: Result<(Prepared, NativeDraft), wire::Error>,
    chain: Option<ChainSnapshot>,
}
/// Wallet and chain identity available at construction. An unchanged poll
/// leaves the draft usable; a new tip or changed selected coin invalidates it.
#[derive(Clone, Copy, PartialEq, Eq)]
struct ChainSnapshot {
    wallet_created_at: u32,
    block_height: i32,
    tip_hash: BlockHash,
}
impl ChainSnapshot {
    fn new(info: &crate::daemon::model::GetInfoResult, tip_hash: BlockHash) -> Self {
        Self {
            wallet_created_at: info.timestamp,
            block_height: info.block_height,
            tip_hash,
        }
    }
}
pub(crate) fn supported_chain(chain: ChainId) -> bool {
    chain.dir_name().parse::<wire::Chain>().is_ok()
}

const STALE_STATUS: &str = "stale_requires_new_request";
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReviewStatus {
    Ready,
    Stale,
    Unavailable,
}
fn stale(draft: &mut Draft) {
    if let Ok((summary, _)) = &mut draft.result {
        summary.status = STALE_STATUS.into();
    }
}
#[derive(Clone)]
pub(crate) struct Service {
    session_id: uuid::Uuid,
    pub(super) chain: ChainId,
    descriptor: CoincubeDescriptor,
    daemon: Arc<dyn Daemon + Send + Sync>,
    wallet_id: String,
    last_poll_at_startup: Option<u32>,
    drafts: Arc<tokio::sync::Mutex<BTreeMap<String, Draft>>>,
    revoked: Arc<AtomicBool>,
    slots: Arc<tokio::sync::Semaphore>,
}
impl Service {
    pub fn new(
        chain: ChainId,
        descriptor: CoincubeDescriptor,
        daemon: Arc<dyn Daemon + Send + Sync>,
        wallet_id: String,
        last_poll_at_startup: Option<u32>,
    ) -> Self {
        Self {
            session_id: uuid::Uuid::new_v4(),
            chain,
            descriptor,
            daemon,
            wallet_id,
            last_poll_at_startup,
            drafts: Arc::default(),
            revoked: Arc::default(),
            slots: Arc::new(tokio::sync::Semaphore::new(1)),
        }
    }
    pub fn revoke(&self) {
        self.revoked.store(true, Ordering::SeqCst);
    }
    pub fn session_id(&self) -> uuid::Uuid {
        self.session_id
    }
    pub fn summaries(&self) -> Vec<Prepared> {
        if self.revoked.load(Ordering::SeqCst) {
            return Vec::new();
        }
        self.drafts
            .try_lock()
            .map(|d| {
                d.values()
                    .filter_map(|v| v.result.as_ref().ok().map(|(s, _)| s.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }
    #[cfg(test)]
    pub fn native(&self, id: &str) -> Option<NativeDraft> {
        if self.revoked.load(Ordering::SeqCst) {
            return None;
        }
        self.drafts
            .try_lock()
            .ok()?
            .values()
            .filter_map(|d| d.result.as_ref().ok())
            .find(|(s, _)| s.draft_id == id && s.status != STALE_STATUS)
            .map(|(_, native)| native.clone())
    }
    /// Check whether a concurrent MCP read invalidated this identity after its
    /// async validation completed. Lock contention is temporary unavailability.
    pub fn review_status(&self, id: &str) -> ReviewStatus {
        if self.revoked.load(Ordering::SeqCst) {
            return ReviewStatus::Unavailable;
        }
        let Ok(drafts) = self.drafts.try_lock() else {
            return ReviewStatus::Unavailable;
        };
        drafts
            .values()
            .filter_map(|draft| draft.result.as_ref().ok())
            .find(|(summary, _)| summary.draft_id == id)
            .map_or(ReviewStatus::Unavailable, |(summary, _)| {
                if summary.status == STALE_STATUS {
                    ReviewStatus::Stale
                } else {
                    ReviewStatus::Ready
                }
            })
    }
    /// Recheck the selected coins and the daemon snapshot immediately before a
    /// native handoff. The returned PSBT is the original immutable construction.
    pub async fn native_checked(&self, id: &str) -> Option<NativeDraft> {
        if self.revoked.load(Ordering::SeqCst) {
            return None;
        }
        let mut drafts = self.drafts.lock().await;
        let draft = drafts.values_mut().find(|d| {
            d.result
                .as_ref()
                .is_ok_and(|(summary, _)| summary.draft_id == id)
        })?;
        let info = match self.daemon.get_info().await {
            Ok(info) => info,
            Err(_) => {
                stale(draft);
                return None;
            }
        };
        if !self.ready(&info) || !self.refresh(draft, &info).await {
            stale(draft);
            return None;
        }
        if self.revoked.load(Ordering::SeqCst) {
            return None;
        }
        draft.result.as_ref().ok().map(|(_, native)| native.clone())
    }
    async fn refresh(&self, draft: &mut Draft, info: &crate::daemon::model::GetInfoResult) -> bool {
        let Ok((summary, native)) = &draft.result else {
            return false;
        };
        if summary.status == STALE_STATUS {
            return false;
        }
        let Ok(tip_before) = self.daemon.chain_tip_hash().await else {
            return false;
        };
        if draft.chain != Some(ChainSnapshot::new(info, tip_before)) {
            return false;
        }
        let outpoints: Vec<_> = native
            .psbt
            .unsigned_tx
            .input
            .iter()
            .map(|input| input.previous_output)
            .collect();
        let Ok(current) = self.daemon.list_coins(&[], &outpoints).await else {
            return false;
        };
        if !selected_inputs_unchanged(&native.coins, &current.coins) {
            return false;
        }
        if validate(
            self.chain,
            &draft.input,
            &native.psbt,
            &current.coins,
            &self.descriptor,
        )
        .is_err()
        {
            return false;
        }
        let Ok(tip_after) = self.daemon.chain_tip_hash().await else {
            return false;
        };
        if tip_after != tip_before {
            return false;
        }
        let Ok(info_after) = self.daemon.get_info().await else {
            return false;
        };
        self.ready(&info_after) && draft.chain == Some(ChainSnapshot::new(&info_after, tip_after))
    }
    fn ready(&self, info: &crate::daemon::model::GetInfoResult) -> bool {
        !(!self.daemon.backend().is_embedded()
            || info.chain_id != Some(self.chain)
            || !supported_chain(self.chain)
            || !info.sync.is_finite()
            || info.sync != 1.0
            || info.network != self.chain.bitcoin_network()
            || info.descriptors.main.as_vault() != Some(&self.descriptor)
            || info.rescan_progress.is_some()
            || info.refused_reorg_depth.is_some()
            || info.chain_divergence
            || !matches!(
                crate::app::wallet::sync_status(
                    self.daemon.backend(),
                    info.block_height,
                    info.sync,
                    info.last_poll_timestamp,
                    self.last_poll_at_startup,
                    &info.history_sync
                ),
                SyncStatus::Synced
            ))
    }
    async fn handle(&self, request: wire::Request) -> Result<wire::Response, wire::Error> {
        use wire::Error;
        if self.revoked.load(Ordering::SeqCst) || !request.valid() {
            return Err(Error::AccessDenied);
        }
        match &request {
            wire::Request::List(_) => {}
            wire::Request::PrepareTransaction(v)
                if v.wallet_id == self.wallet_id && v.network.as_str() == self.chain.dir_name() => {
            }
            wire::Request::Transaction(v) if v.wallet_id == self.wallet_id => {}
            _ => return Err(Error::AccessDenied),
        }
        let info = match self.daemon.get_info().await {
            Ok(info) => info,
            Err(_) => {
                let mut drafts = self.drafts.lock().await;
                for draft in drafts.values_mut() {
                    stale(draft);
                }
                return Err(Error::Unavailable);
            }
        };
        if !self.ready(&info) {
            let mut drafts = self.drafts.lock().await;
            for draft in drafts.values_mut() {
                stale(draft);
            }
            return Err(Error::Unavailable);
        }
        match request {
            wire::Request::List(_) => Ok(wire::Response::List(wire::WalletList {
                wallets: vec![wire::WalletMetadata {
                    network: self.chain.to_string(),
                    wallet_id: self.wallet_id.clone(),
                    display_name: "Vault".into(),
                    wallet_type: "vault".into(),
                }],
            })),
            wire::Request::PrepareTransaction(input) => self
                .prepare(input, &info)
                .await
                .map(wire::Response::Transaction),
            wire::Request::Transaction(DraftInput { draft_id, .. }) => {
                let mut drafts = self.drafts.lock().await;
                let draft = drafts
                    .values_mut()
                    .find(|d| d.result.as_ref().is_ok_and(|(s, _)| s.draft_id == draft_id))
                    .ok_or(Error::AccessDenied)?;
                if !self.refresh(draft, &info).await {
                    stale(draft);
                }
                Ok(wire::Response::Transaction(
                    draft.result.as_ref().expect("matched success").0.clone(),
                ))
            }
            _ => Err(Error::AccessDenied),
        }
    }
    async fn prepare(
        &self,
        input: PrepareInput,
        info: &crate::daemon::model::GetInfoResult,
    ) -> Result<Prepared, wire::Error> {
        use wire::Error;
        // Held only on the single bounded blocking worker. A timed-out caller
        // can retry the stored construction instead of reserving change twice.
        let mut drafts = self.drafts.lock().await;
        if let Some(draft) = drafts.get_mut(&input.request_id) {
            return if draft.input == input {
                if draft.result.is_ok() && !self.refresh(draft, info).await {
                    stale(draft);
                }
                draft
                    .result
                    .as_ref()
                    .map(|(s, _)| s.clone())
                    .map_err(Clone::clone)
            } else {
                Err(Error::TransactionConflict)
            };
        }
        if drafts.len() >= wire::transaction::MAX_DRAFTS {
            return Err(Error::Unavailable);
        }
        let mut destinations = HashMap::new();
        for recipient in &input.recipients {
            let address = Address::from_str(&recipient.address)
                .map_err(|_| Error::InvalidInput)?
                .require_network(self.chain.bitcoin_network())
                .map_err(|_| Error::InvalidInput)?;
            if output_below_floor(recipient.amount_sats, address.script_pubkey())
                || destinations
                    .insert(address.into_unchecked(), recipient.amount_sats)
                    .is_some()
            {
                return Err(Error::InvalidInput);
            }
        }
        let tip_before = self
            .daemon
            .chain_tip_hash()
            .await
            .map_err(|_| Error::Unavailable)?;
        // Charge and cache every engine attempt, including fee/validation failures:
        // successful engine construction may already have reserved change. Keep
        // this lock across completion so cancellation/retries cannot build twice.
        drafts.insert(
            input.request_id.clone(),
            Draft {
                input: input.clone(),
                result: Err(Error::Unavailable),
                chain: None,
            },
        );
        let result = self.construct(&input, &destinations).await;
        let draft = drafts.get_mut(&input.request_id).expect("attempt inserted");
        draft.chain = result
            .as_ref()
            .ok()
            .map(|_| ChainSnapshot::new(info, tip_before));
        draft.result = result;
        if draft.result.is_ok() && !self.refresh(draft, info).await {
            stale(draft);
        }
        draft
            .result
            .as_ref()
            .map(|(s, _)| s.clone())
            .map_err(Clone::clone)
    }
    async fn construct(
        &self,
        input: &PrepareInput,
        destinations: &HashMap<
            Address<coincube_core::miniscript::bitcoin::address::NetworkUnchecked>,
            u64,
        >,
    ) -> Result<(Prepared, NativeDraft), wire::Error> {
        use wire::Error;
        let result = match self
            .daemon
            .create_spend_tx_capped(
                &[],
                destinations,
                input.fee_rate_sat_vb,
                None,
                input.max_fee_sats,
            )
            .await
        {
            Ok(result) => result,
            Err(crate::daemon::DaemonError::FeeCeilingExceeded) => {
                return Err(Error::FeeLimitExceeded);
            }
            Err(crate::daemon::DaemonError::FeeRateExceeded) => {
                return Err(Error::FeeRateExceeded);
            }
            Err(_) => return Err(Error::Unavailable),
        };
        let (mut psbt, warnings) = match result {
            CreateSpendResult::Success { psbt, warnings } => (psbt, warnings),
            CreateSpendResult::InsufficientFunds { .. } => return Err(Error::InsufficientFunds),
        };
        let outpoints: Vec<_> = psbt
            .unsigned_tx
            .input
            .iter()
            .map(|i| i.previous_output)
            .collect();
        let coins = self
            .daemon
            .list_coins(&[], &outpoints)
            .await
            .map_err(|_| Error::Unavailable)?
            .coins;
        let (fee_sats, change_sats, fee_rate_sat_vb) =
            validate(self.chain, input, &psbt, &coins, &self.descriptor)?;
        // The returned rate is the maximum-size estimate. Broadcast checks
        // the requested rate against that same prepared maximum signed size.
        coincube_core::spend::stamp_mcp_fee_cap(
            &mut psbt,
            input.max_fee_sats,
            input.fee_rate_sat_vb,
        );
        if self.revoked.load(Ordering::SeqCst) {
            return Err(Error::AccessDenied);
        }
        let summary = Prepared {
            wallet_id: self.wallet_id.clone(),
            draft_id: uuid::Uuid::new_v4().to_string(),
            network: self.chain.to_string(),
            status: "unsigned_ready_for_native_review".into(),
            recipients: input.recipients.clone(),
            amount_sats: input.recipients.iter().map(|r| r.amount_sats).sum(),
            fee_sats,
            fee_rate_sat_vb,
            change_sats,
            input_count: outpoints.len(),
            signing_required: true,
        };
        Ok((
            summary,
            NativeDraft {
                chain: self.chain,
                psbt,
                coins,
                warnings,
            },
        ))
    }
}

fn selected_inputs_unchanged(original: &[Coin], current: &[Coin]) -> bool {
    let mut seen = HashSet::new();
    original.len() == current.len()
        && current.iter().all(|coin| {
            seen.insert(coin.outpoint)
                && original.iter().any(|old| {
                    old.outpoint == coin.outpoint
                        && old.amount == coin.amount
                        && old.block_height == coin.block_height
                        && old.derivation_index == coin.derivation_index
                        && old.is_change == coin.is_change
                        && old.is_from_self == coin.is_from_self
                        && old.spend_info.is_none()
                        && coin.spend_info.is_none()
                        && !coin.is_immature
                })
        })
}
impl wire::Reader for Service {
    fn read(
        &self,
        request: wire::Request,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<wire::Response, wire::Error>> + Send + '_>,
    > {
        Box::pin(async move {
            let permit = self
                .slots
                .clone()
                .try_acquire_owned()
                .map_err(|_| wire::Error::Unavailable)?;
            let service = self.clone();
            tokio::task::spawn_blocking(move || {
                let _permit = permit;
                tokio::runtime::Handle::current().block_on(service.handle(request))
            })
            .await
            .map_err(|_| wire::Error::Unavailable)?
        })
    }
}

/// MCP drafts use the network dust minimum for each output script.
fn output_below_floor(
    amount_sats: u64,
    script: coincube_core::miniscript::bitcoin::ScriptBuf,
) -> bool {
    amount_sats < script.minimal_non_dust().to_sat()
}

/// Independently verify the engine's public construction before exposing a summary.
fn validate(
    chain: ChainId,
    input: &PrepareInput,
    psbt: &Psbt,
    coins: &[Coin],
    descriptor: &CoincubeDescriptor,
) -> Result<(u64, u64, u64), wire::Error> {
    use wire::Error;
    if input.network.as_str() != chain.dir_name()
        || !supported_chain(chain)
        || psbt.serialize().len() > 65_536
        || psbt.inputs.is_empty()
        || psbt.inputs.len() > 128
        || psbt.inputs.len() != psbt.unsigned_tx.input.len()
        || psbt.outputs.len() != psbt.unsigned_tx.output.len()
        || psbt.inputs.iter().any(|i| {
            !i.partial_sigs.is_empty()
                || i.final_script_sig.is_some()
                || i.final_script_witness.is_some()
                || i.tap_key_sig.is_some()
                || !i.tap_script_sigs.is_empty()
        })
        || psbt.unsigned_tx.input.iter().any(|i| {
            !i.script_sig.is_empty() || !i.witness.is_empty() || i.sequence.is_relative_lock_time()
        })
    {
        return Err(Error::Unavailable);
    }
    // Fork signatures are proprietary input records, not partial_sigs. A
    // construction must be unsigned in both representations before disclosure.
    if chain.is_blake2b() {
        use coincube_core::psbt_unified::{unified_signatures, UnifiedPsbt};
        let unified = UnifiedPsbt::from_psbt(psbt.clone()).map_err(|_| Error::Unavailable)?;
        if !unified_signatures(&unified)
            .map_err(|_| Error::Unavailable)?
            .is_empty()
        {
            return Err(Error::Unavailable);
        }
    }
    let secp = secp256k1::Secp256k1::verification_only();
    let total_in = validated_input_total(psbt, coins, descriptor, &secp)?;
    let mut expected = BTreeMap::new();
    for r in &input.recipients {
        let script = Address::from_str(&r.address)
            .map_err(|_| Error::InvalidInput)?
            .require_network(chain.bitcoin_network())
            .map_err(|_| Error::InvalidInput)?
            .script_pubkey();
        if expected.insert(script, r.amount_sats).is_some() {
            return Err(Error::InvalidInput);
        }
    }
    let (total_out, change) = validated_output_totals(psbt, descriptor, &secp, expected)?;
    let fee = total_in
        .checked_sub(total_out)
        .filter(|f| *f > 0)
        .ok_or(Error::Unavailable)?;
    if fee > input.max_fee_sats {
        return Err(Error::FeeLimitExceeded);
    }
    // Match the spend engine's maximum signed size, including SegWit framing.
    let vsize = descriptor.unsigned_tx_max_vbytes(&psbt.unsigned_tx, true);
    if vsize == 0 {
        return Err(Error::Unavailable);
    }
    let measured_rate = fee.div_ceil(vsize);
    if measured_rate > input.fee_rate_sat_vb {
        return Err(Error::FeeRateExceeded);
    }
    Ok((fee, change, measured_rate))
}

fn validated_input_total(
    psbt: &Psbt,
    coins: &[Coin],
    descriptor: &CoincubeDescriptor,
    secp: &secp256k1::Secp256k1<secp256k1::VerifyOnly>,
) -> Result<u64, wire::Error> {
    use wire::Error;
    let mut seen = HashSet::new();
    let mut total_in = 0u64;
    for (txin, metadata) in psbt.unsigned_tx.input.iter().zip(&psbt.inputs) {
        let coin = coins
            .iter()
            .find(|c| c.outpoint == txin.previous_output)
            .ok_or(Error::Unavailable)?;
        let own = if coin.is_change {
            descriptor.change_descriptor()
        } else {
            descriptor.receive_descriptor()
        }
        .derive(coin.derivation_index, secp)
        .script_pubkey();
        let prevout = metadata.witness_utxo.as_ref().ok_or(Error::Unavailable)?;
        if !seen.insert(coin.outpoint)
            || coin.spend_info.is_some()
            || coin.is_immature
            || (coin.block_height.is_none() && !coin.is_from_self)
            || prevout.value != coin.amount
            || prevout.script_pubkey != own
        {
            return Err(Error::Unavailable);
        }
        total_in = total_in
            .checked_add(coin.amount.to_sat())
            .filter(|v| *v <= wire::transaction::MAX_SATS)
            .ok_or(Error::Unavailable)?;
    }
    Ok(total_in)
}

fn validated_output_totals(
    psbt: &Psbt,
    descriptor: &CoincubeDescriptor,
    secp: &secp256k1::Secp256k1<secp256k1::VerifyOnly>,
    mut expected: BTreeMap<coincube_core::miniscript::bitcoin::ScriptBuf, u64>,
) -> Result<(u64, u64), wire::Error> {
    use wire::Error;
    let change_indexes = descriptor.change_indexes(psbt, secp);
    let mut total_out = 0u64;
    let mut change = 0u64;
    let mut changes = 0;
    for (n, output) in psbt.unsigned_tx.output.iter().enumerate() {
        if output_below_floor(output.value.to_sat(), output.script_pubkey.clone()) {
            return Err(Error::Unavailable);
        }
        let is_own = change_indexes.iter().any(|c| c.index() == n);
        if let Some(amount) = expected.remove(&output.script_pubkey) {
            // Self-send is outside this recipient-payment experiment. Existing
            // UI classifies owned outputs as change, which would hide this payment.
            if output.value.to_sat() != amount || is_own {
                return Err(Error::InvalidInput);
            }
        } else if is_own {
            changes += 1;
            if changes > 1 {
                return Err(Error::Unavailable);
            }
            change = output.value.to_sat();
        } else {
            return Err(Error::Unavailable);
        }
        total_out = total_out
            .checked_add(output.value.to_sat())
            .filter(|v| *v <= wire::transaction::MAX_SATS)
            .ok_or(Error::Unavailable)?;
    }
    if !expected.is_empty() {
        return Err(Error::Unavailable);
    }
    Ok((total_out, change))
}

#[cfg(test)]
mod tests {
    use super::*;
    use coincube_core::miniscript::bitcoin::Network;
    use coincube_core::{
        descriptors::{CoincubePolicy, PathInfo},
        miniscript::{
            bitcoin::{hashes::Hash, Amount, ScriptBuf, WScriptHash},
            descriptor::DescriptorPublicKey,
        },
    };
    use wire::Reader;

    async fn fixture() -> (std::path::PathBuf, Service) {
        fixture_on_chain(ChainId::Regtest).await
    }
    pub(super) async fn fixture_on_chain(chain: ChainId) -> (std::path::PathBuf, Service) {
        let owner = DescriptorPublicKey::from_str("[aabbccdd]xpub68JJTXc1MWK8KLW4HGLXZBJknja7kDUJuFHnM424LbziEXsfkh1WQCiEjjHw4zLqSUm4rvhgyGkkuRowE9tCJSgt3TQB5J3SKAbZ2SdcKST/<0;1>/*").unwrap();
        let recovery = DescriptorPublicKey::from_str("[aabbccdd]xpub68JJTXc1MWK8PEQozKsRatrUHXKFNkD1Cb1BuQU9Xr5moCv87anqGyXLyUd4KpnDyZgo3gz4aN1r3NiaoweFW8UutBsBbgKHzaD5HkTkifK/<0;1>/*").unwrap();
        // Re-encode the existing public fixture accounts for the selected chain.
        // Their public key/chain code are unchanged; no signing keys are generated.
        let testnet = |mut key: DescriptorPublicKey| {
            if let DescriptorPublicKey::MultiXPub(ref mut account) = key {
                account.xkey.network = chain.bitcoin_network().into();
            }
            key
        };
        let descriptor = CoincubeDescriptor::new(
            CoincubePolicy::new_legacy(
                PathInfo::Single(testnet(owner)),
                [(144, PathInfo::Single(testnet(recovery)))]
                    .iter()
                    .cloned()
                    .collect(),
            )
            .unwrap(),
        );
        let root =
            std::env::temp_dir().join(format!("mcp-transaction-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let (config, handle) =
            coincubed::mcp_test_fixture::wallet_on_chain(&root, descriptor.clone(), chain);
        let daemon =
            Arc::new(crate::daemon::embedded::EmbeddedDaemon::from_regtest_handle(config, handle));
        let service = Service::new(chain, descriptor, daemon, "wallet".into(), None);
        // The poller updates its sync-progress cache after DaemonHandle::start.
        // Wait for the same complete readiness state required by MCP requests.
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let info = service.daemon.get_info().await.unwrap();
            if service.ready(&info) {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "fixture not synced: height={} progress={} last_poll={:?} history_scan={} rescan={:?}",
                info.block_height,
                info.sync,
                info.last_poll_timestamp,
                info.history_sync.full_scan_in_progress,
                info.rescan_progress
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        (root, service)
    }
    pub(super) fn input() -> PrepareInput {
        let script = ScriptBuf::new_p2wsh(&WScriptHash::all_zeros());
        PrepareInput {
            network: wire::Chain::Regtest,
            wallet_id: "wallet".into(),
            request_id: "payment-1".into(),
            recipients: vec![wire::transaction::Recipient {
                address: Address::from_script(&script, Network::Regtest)
                    .unwrap()
                    .to_string(),
                amount_sats: 10_000,
            }],
            fee_rate_sat_vb: 2,
            max_fee_sats: 1_000,
        }
    }
    async fn prepared(service: &Service, input: PrepareInput) -> Prepared {
        match service
            .read(wire::Request::PrepareTransaction(input))
            .await
            .unwrap()
        {
            wire::Response::Transaction(v) => v,
            _ => panic!("expected transaction summary"),
        }
    }
    pub(super) async fn cleanup(root: std::path::PathBuf, service: Service) {
        service.daemon.stop().await.unwrap();
        drop(service);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn mcp_transaction_real_engine_retry_conflict_scope_and_no_saved_spend() {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let (root, service) = fixture().await;
            let first = prepared(&service, input()).await;
            assert_eq!(first.amount_sats,10_000); assert_eq!(first.network,"regtest");
            assert!(first.fee_sats > 0 && first.fee_sats <= 1_000 && first.signing_required);
            assert_eq!(first.amount_sats+first.fee_sats+first.change_sats,55_000);
            let index = service.daemon.get_info().await.unwrap().change_index;
            assert_eq!(prepared(&service,input()).await,first);
            assert_eq!(service.daemon.get_info().await.unwrap().change_index,index,"retry reserved another change address");
            let mut changed=input();changed.recipients[0].amount_sats+=1;
            assert!(matches!(service.read(wire::Request::PrepareTransaction(changed)).await,Err(wire::Error::TransactionConflict)));
            let get = DraftInput {wallet_id:"wallet".into(),draft_id:first.draft_id.clone()};
            assert!(matches!(service.read(wire::Request::Transaction(get)).await,Ok(wire::Response::Transaction(v))if v==first));
            assert!(service.daemon.list_spend_txs().await.unwrap().spend_txs.is_empty(),"MCP saved a spend");
            assert!(service.native(&first.draft_id).unwrap().psbt.inputs.iter().all(|i|i.partial_sigs.is_empty()&&i.tap_key_sig.is_none()));
            for request in [wire::Request::Summary(wire::WalletInput {wallet_id:"wallet".into()}),wire::Request::Transaction(DraftInput{wallet_id:"other".into(),draft_id:first.draft_id.clone()}),wire::Request::Transaction(DraftInput{wallet_id:"wallet".into(),draft_id:"foreign".into()})] {
                assert!(matches!(service.read(request).await,Err(wire::Error::AccessDenied)));
            }
            service.revoke();assert!(service.native(&first.draft_id).is_none());
            assert!(matches!(service.read(wire::Request::PrepareTransaction(input())).await,Err(wire::Error::AccessDenied)));
            cleanup(root,service).await;
        });
    }
    #[test]
    fn mcp_transaction_draft_freshness_requires_new_identity() {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let (root, service) = fixture().await;
            let first = prepared(&service, input()).await;
            let native = service.native_checked(&first.draft_id).await.unwrap();
            assert_eq!(format!("{native:?}"), "NativeDraft(<redacted>)");
            assert!(selected_inputs_unchanged(&native.coins, &native.coins));
            assert!(!selected_inputs_unchanged(&native.coins, &[]));
            let mut spent = native.coins.clone();
            spent[0].spend_info = Some(coincubed::commands::LCSpendInfo {
                txid: coincube_core::miniscript::bitcoin::Txid::all_zeros(),
                height: None,
            });
            assert!(!selected_inputs_unchanged(&native.coins, &spent));
            let mut immature = native.coins.clone();
            immature[0].is_immature = true;
            assert!(!selected_inputs_unchanged(&native.coins, &immature));
            let mut reorged = native.coins.clone();
            reorged[0].block_height = Some(2);
            assert!(!selected_inputs_unchanged(&native.coins, &reorged));

            // Simulate a later daemon poll or wallet replacement. A request
            // identity remains charged, and its native PSBT can no longer open.
            let before = service.daemon.get_info().await.unwrap().change_index;
            {
                let mut drafts = service.drafts.lock().await;
                let snapshot = drafts
                    .get_mut(&input().request_id)
                    .unwrap()
                    .chain
                    .as_mut()
                    .unwrap();
                snapshot.wallet_created_at = snapshot.wallet_created_at.wrapping_add(1);
            }
            let get = wire::Request::Transaction(DraftInput {
                wallet_id: "wallet".into(),
                draft_id: first.draft_id.clone(),
            });
            let wire::Response::Transaction(stale_summary) = service.read(get).await.unwrap()
            else {
                panic!("expected stale summary")
            };
            assert_eq!(stale_summary.status, STALE_STATUS);
            assert!(matches!(
                service.review_status(&first.draft_id),
                ReviewStatus::Stale
            ));
            assert!(service.native_checked(&first.draft_id).await.is_none());
            assert!(service.native(&first.draft_id).is_none());
            assert_eq!(prepared(&service, input()).await, stale_summary);
            assert_eq!(
                service.daemon.get_info().await.unwrap().change_index,
                before
            );
            let serialized = serde_json::to_string(&stale_summary).unwrap();
            assert!(!serialized.contains("psbt"));
            assert!(!serialized.contains("outpoint"));
            assert!(!serialized.contains("change_address"));
            let mut fresh = input();
            fresh.request_id = "payment-2".into();
            let replacement = prepared(&service, fresh).await;
            assert_ne!(replacement.draft_id, first.draft_id);
            assert_eq!(replacement.status, "unsigned_ready_for_native_review");
            cleanup(root, service).await;
        });
    }
    #[test]
    fn mcp_transaction_noop_poll_keeps_prepared_draft_reviewable() {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let (root, service) = fixture().await;
            let first = prepared(&service, input()).await;
            let before = service.daemon.get_info().await.unwrap();
            let tip = service.daemon.chain_tip_hash().await.unwrap();
            // The fixture uses a real poller but normally sleeps for an hour.
            // Wake it after the whole-second timestamp can advance.
            tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
            service.daemon.request_sync().await.unwrap();
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
            let after = loop {
                let info = service.daemon.get_info().await.unwrap();
                if info.last_poll_timestamp != before.last_poll_timestamp && service.ready(&info) {
                    break info;
                }
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "no-op poll did not complete"
                );
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            };
            assert_eq!(after.block_height, before.block_height);
            assert_eq!(service.daemon.chain_tip_hash().await.unwrap(), tip);
            assert_eq!(prepared(&service, input()).await, first);
            let get = wire::Request::Transaction(DraftInput {
                wallet_id: "wallet".into(),
                draft_id: first.draft_id.clone(),
            });
            assert!(
                matches!(service.read(get).await, Ok(wire::Response::Transaction(v)) if v == first)
            );
            assert!(service.native_checked(&first.draft_id).await.is_some());
            assert!(matches!(
                service.review_status(&first.draft_id),
                ReviewStatus::Ready
            ));
            assert_eq!(
                service.daemon.get_info().await.unwrap().change_index,
                after.change_index
            );
            cleanup(root, service).await;
        });
    }
    #[test]
    fn mcp_transaction_missing_input_is_stale_before_native_review() {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let (root, service) = fixture().await;
            let summary = prepared(&service, input()).await;
            let outpoint = service.native(&summary.draft_id).unwrap().coins[0].outpoint;
            coincubed::mcp_test_fixture::remove_coin(&root, outpoint);
            assert!(service.native_checked(&summary.draft_id).await.is_none());
            let get = wire::Request::Transaction(DraftInput {
                wallet_id: "wallet".into(),
                draft_id: summary.draft_id.clone(),
            });
            let wire::Response::Transaction(status) = service.read(get).await.unwrap() else {
                panic!("expected transaction summary")
            };
            assert_eq!(status.status, STALE_STATUS);
            assert!(matches!(
                service.review_status(&summary.draft_id),
                ReviewStatus::Stale
            ));
            assert_eq!(prepared(&service, input()).await, status);
            cleanup(root, service).await;
        });
    }
    #[test]
    fn mcp_transaction_same_height_reorg_invalidates_draft() {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let (root, service) = fixture().await;
            let summary = prepared(&service, input()).await;
            let before = service.daemon.get_info().await.unwrap();
            let hash_before = service.daemon.chain_tip_hash().await.unwrap();
            coincubed::mcp_test_fixture::replace_tip_hash_same_height(&root);
            let after = service.daemon.get_info().await.unwrap();
            assert_eq!(after.block_height, before.block_height);
            assert_eq!(after.last_poll_timestamp, before.last_poll_timestamp);
            assert_ne!(service.daemon.chain_tip_hash().await.unwrap(), hash_before);
            assert!(service.native_checked(&summary.draft_id).await.is_none());
            let get = wire::Request::Transaction(DraftInput {
                wallet_id: "wallet".into(),
                draft_id: summary.draft_id.clone(),
            });
            assert!(matches!(service.read(get).await, Ok(wire::Response::Transaction(v)) if v.status == STALE_STATUS));
            cleanup(root, service).await;
        });
    }
    #[test]
    fn mcp_transaction_engine_refuses_bad_network_dust_duplicates_insufficiency_and_fee_cap() {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let (root,service) = fixture().await;
            let mut cases=Vec::new();
            let mut bad=input();bad.recipients[0].address=Address::from_script(&ScriptBuf::new_p2wsh(&WScriptHash::all_zeros()),Network::Bitcoin).unwrap().to_string();cases.push((bad,wire::Error::InvalidInput));
            let mut dust=input();dust.recipients[0].amount_sats=1;cases.push((dust,wire::Error::InvalidInput));
            let mut duplicate=input();duplicate.recipients.push(duplicate.recipients[0].clone());cases.push((duplicate,wire::Error::InvalidInput));
            let mut insufficient=input();insufficient.recipients[0].amount_sats=55_000;cases.push((insufficient,wire::Error::InsufficientFunds));
            let mut fee=input();fee.max_fee_sats=1;cases.push((fee,wire::Error::FeeLimitExceeded));
            for (i,(mut request,expected))in cases.into_iter().enumerate(){request.request_id=format!("rejected-{i}");assert!(matches!(service.read(wire::Request::PrepareTransaction(request)).await,Err(e)if e==expected));}
            assert!(service.summaries().is_empty());
            for i in 0..wire::transaction::MAX_DRAFTS-2 {let mut request=input();request.request_id=format!("bounded-{i}");prepared(&service,request).await;}
            let mut overflow=input();overflow.request_id="overflow".into();assert!(matches!(service.read(wire::Request::PrepareTransaction(overflow)).await,Err(wire::Error::Unavailable)));
            assert!(service.daemon.list_spend_txs().await.unwrap().spend_txs.is_empty());
            cleanup(root,service).await;
        });
    }
    #[test]
    fn mcp_transaction_construction_checks_reject_changed_outputs_signatures_inputs_and_fee() {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let (root, service) = fixture().await;
            let summary = prepared(&service, input()).await;
            let native = service.native(&summary.draft_id).unwrap();
            assert_eq!(
                validate(
                    ChainId::Regtest,
                    &input(),
                    &native.psbt,
                    &native.coins,
                    &service.descriptor
                )
                .unwrap(),
                (
                    summary.fee_sats,
                    summary.change_sats,
                    summary.fee_rate_sat_vb
                )
            );
            let mut one_sat_per_vbyte = native.psbt.clone();
            let max_vsize = service
                .descriptor
                .unsigned_tx_max_vbytes(&one_sat_per_vbyte.unsigned_tx, true);
            let total_in: u64 = native.coins.iter().map(|coin| coin.amount.to_sat()).sum();
            let recipient_script = Address::from_str(&input().recipients[0].address)
                .unwrap()
                .assume_checked()
                .script_pubkey();
            let change = one_sat_per_vbyte
                .unsigned_tx
                .output
                .iter_mut()
                .find(|output| output.script_pubkey != recipient_script)
                .unwrap();
            let change_sats = total_in - input().recipients[0].amount_sats - max_vsize;
            change.value = Amount::from_sat(change_sats);
            let mut one_sat_request = input();
            one_sat_request.fee_rate_sat_vb = 1;
            assert_eq!(
                validate(
                    ChainId::Regtest,
                    &one_sat_request,
                    &one_sat_per_vbyte,
                    &native.coins,
                    &service.descriptor
                ),
                Ok((max_vsize, change_sats, 1))
            );
            let mut cases = Vec::new();
            let mut signed = native.psbt.clone();
            signed.inputs[0].final_script_sig = Some(ScriptBuf::new());
            cases.push(signed);
            let mut value = native.psbt.clone();
            value.inputs[0].witness_utxo.as_mut().unwrap().value += Amount::from_sat(1);
            cases.push(value);
            let mut recipient = native.psbt.clone();
            let target = Address::from_str(&input().recipients[0].address)
                .unwrap()
                .assume_checked()
                .script_pubkey();
            recipient
                .unsigned_tx
                .output
                .iter_mut()
                .find(|o| o.script_pubkey == target)
                .unwrap()
                .value += Amount::from_sat(1);
            cases.push(recipient);
            let mut change = native.psbt.clone();
            change
                .unsigned_tx
                .output
                .iter_mut()
                .find(|o| o.script_pubkey != target)
                .unwrap()
                .script_pubkey = ScriptBuf::new();
            cases.push(change);
            let mut recovery = native.psbt.clone();
            recovery.unsigned_tx.input[0].sequence =
                coincube_core::miniscript::bitcoin::Sequence::from_height(144);
            cases.push(recovery);
            for psbt in cases {
                assert!(validate(
                    ChainId::Regtest,
                    &input(),
                    &psbt,
                    &native.coins,
                    &service.descriptor
                )
                .is_err());
            }
            let mut cap = input();
            cap.max_fee_sats = 1;
            assert_eq!(
                validate(
                    ChainId::Regtest,
                    &cap,
                    &native.psbt,
                    &native.coins,
                    &service.descriptor
                ),
                Err(wire::Error::FeeLimitExceeded)
            );
            let mut spent = native.coins.clone();
            spent[0].is_immature = true;
            assert!(validate(
                ChainId::Regtest,
                &input(),
                &native.psbt,
                &spent,
                &service.descriptor
            )
            .is_err());
            cleanup(root, service).await;
        });
    }

    #[test]
    fn mcp_transaction_native_review_renders_unsigned_change_without_expansion() {
        use iced::advanced::{layout, widget, Layout};
        struct Texts(Vec<(String, iced::Rectangle)>);
        impl widget::Operation for Texts {
            fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn widget::Operation)) {
                operate(self);
            }
            fn text(&mut self, _: Option<&widget::Id>, bounds: iced::Rectangle, text: &str) {
                self.0.push((text.to_string(), bounds));
            }
        }
        let render = |tx: &crate::daemon::model::SpendTx| {
            let mut element: iced::Element<
                '_,
                crate::app::view::Message,
                coincube_ui::theme::Theme,
                (),
            > = crate::app::view::vault::spend::mcp_prepared_review(tx, Network::Regtest);
            let mut tree = widget::Tree::new(element.as_widget());
            let node = element.as_widget_mut().layout(
                &mut tree,
                &(),
                &layout::Limits::new(iced::Size::ZERO, iced::Size::new(800.0, 2000.0)),
            );
            assert!(node.bounds().width >= 800.0);
            let mut operation = Texts(Vec::new());
            element
                .as_widget_mut()
                .operate(&mut tree, Layout::new(&node), &(), &mut operation);
            operation.0
        };
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let (root, service) = fixture().await;
            let summary = prepared(&service, input()).await;
            let native = service.native(&summary.draft_id).unwrap();
            let tx = crate::daemon::model::SpendTx::new(
                None,
                native.psbt,
                native.coins,
                &service.descriptor,
                &secp256k1::Secp256k1::verification_only(),
                Network::Regtest,
            );
            let change = tx
                .psbt
                .unsigned_tx
                .output
                .iter()
                .enumerate()
                .find(|(i, _)| tx.change_indexes.contains(i))
                .unwrap()
                .1;
            let address = Address::from_script(&change.script_pubkey, Network::Regtest)
                .unwrap()
                .to_string();
            let texts = render(&tx);
            for expected in [
                "Unsigned transaction. Your signature is required.".to_string(),
                "Input total: 55000 sats".to_string(),
                "Payment total: 10000 sats".to_string(),
                format!("Miner fee: {} sats", summary.fee_sats),
                format!(
                    "Change returned to this Vault: {} sats",
                    change.value.to_sat()
                ),
                format!("Change address: {address}"),
            ] {
                assert!(
                    texts.iter().any(|(text, _)| text == &expected),
                    "not present in ordinary widget traversal: {}",
                    expected
                );
            }
            assert!(texts.iter().any(|(text, bounds)| text
                == &format!("Change address: {address}")
                && bounds.width > 700.0));
            let mut with_signature = tx.clone();
            with_signature.psbt.inputs[0].final_script_sig = Some(ScriptBuf::new());
            let signed_texts = render(&with_signature);
            assert!(signed_texts.iter().any(
                |(s, _)| s == "Signatures present. Review signing status in COINCUBE Desktop."
            ));
            assert!(!signed_texts
                .iter()
                .any(|(s, _)| s == "Unsigned transaction. Your signature is required."));
            cleanup(root, service).await;
        });
    }

    #[test]
    fn mcp_transaction_startup_baseline_regression() {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let (root, mut service) = fixture().await;
            let mut info = service.daemon.get_info().await.unwrap();
            service.last_poll_at_startup = Some(100);
            info.last_poll_timestamp = Some(100);
            assert!(
                !service.ready(&info),
                "persisted prior-run poll is not a fresh poll"
            );
            info.last_poll_timestamp = Some(101);
            assert!(service.ready(&info));
            cleanup(root, service).await;
        });
    }

    #[test]
    fn mcp_transaction_rejected_constructions_are_cached_and_bounded_regression() {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let (root, service) = fixture().await;
            let mut request = input();
            request.max_fee_sats = 1;
            let call = |v| wire::Request::PrepareTransaction(v);
            let before = service.daemon.get_info().await.unwrap().change_index;
            assert_eq!(
                service.read(call(request.clone())).await.unwrap_err(),
                wire::Error::FeeLimitExceeded
            );
            assert_eq!(
                service.daemon.get_info().await.unwrap().change_index,
                before,
                "fee rejection reserved a change address"
            );
            assert_eq!(
                service.read(call(request.clone())).await.unwrap_err(),
                wire::Error::FeeLimitExceeded
            );
            let index = service.daemon.get_info().await.unwrap().change_index;
            assert_eq!(index, before, "fee rejection reserved a change address");
            assert_eq!(
                service.daemon.get_info().await.unwrap().change_index,
                index,
                "failed retry reserved change again"
            );
            let mut changed = request.clone();
            changed.max_fee_sats = 1000;
            assert_eq!(
                service.read(call(changed)).await.unwrap_err(),
                wire::Error::TransactionConflict
            );
            for i in 1..wire::transaction::MAX_DRAFTS {
                request.request_id = format!("failed-{i}");
                assert_eq!(
                    service.read(call(request.clone())).await.unwrap_err(),
                    wire::Error::FeeLimitExceeded
                );
            }
            let bounded = service.daemon.get_info().await.unwrap().change_index;
            request.request_id = "one-too-many".into();
            assert_eq!(
                service.read(call(request)).await.unwrap_err(),
                wire::Error::Unavailable
            );
            assert_eq!(
                service.daemon.get_info().await.unwrap().change_index,
                bounded
            );
            assert_eq!(bounded, before, "rejected drafts reserved change");
            assert!(service.summaries().is_empty());
            assert!(service
                .daemon
                .list_spend_txs()
                .await
                .unwrap()
                .spend_txs
                .is_empty());
            cleanup(root, service).await;
        });
    }

    #[test]
    fn mcp_transaction_chain_sync_and_reorg_guards_fail_closed() {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let (root, service) = fixture().await;
            let info = service.daemon.get_info().await.unwrap();
            assert!(service.ready(&info));
            let mut cases = Vec::new();
            let mut v = info.clone();
            v.chain_id = Some(ChainId::Bitcoin);
            cases.push(v);
            let mut v = info.clone();
            v.chain_id = None;
            cases.push(v);
            let mut v = info.clone();
            v.network = Network::Bitcoin;
            cases.push(v);
            for progress in [0.0, 0.5, f64::NAN, f64::INFINITY] {
                let mut v = info.clone();
                v.sync = progress;
                cases.push(v);
            }
            let mut v = info.clone();
            v.rescan_progress = Some(0.5);
            cases.push(v);
            let mut v = info.clone();
            v.refused_reorg_depth = Some(100);
            cases.push(v);
            let mut v = info.clone();
            v.chain_divergence = true;
            cases.push(v);
            let mut v = info.clone();
            v.last_poll_timestamp = None;
            cases.push(v);
            for v in cases {
                assert!(!service.ready(&v));
            }
            cleanup(root, service).await;
        });
    }

    #[test]
    fn mcp_transaction_native_pairing_binds_wallet_and_loses_drafts_on_lock_and_revoke() {
        use super::super::{pairings, Message, PairingContext, Settings};
        use crate::app::{session, wallet::Wallet};
        let _guard = session::test_guard();
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let (root, service) = fixture().await;
            let wallet =
                Arc::new(Wallet::new(service.descriptor.clone()).with_chain(ChainId::Regtest));
            std::fs::create_dir_all(root.join("regtest")).unwrap();
            let location = pairings::Location::new(&root, ChainId::Regtest, "tx-fixture").unwrap();
            // Unit-only consent and registry wrapping key; signed native consent
            // is tested separately and is never inferred from this fixture.
            let attach = |create| Settings {
                enabled: true,
                pairing_location: Some(location.clone()),
                pairing_store: Some(
                    pairings::Store::open(
                        location.clone(),
                        zeroize::Zeroizing::new([31; 32]),
                        create,
                    )
                    .unwrap(),
                ),
                ..Settings::default()
            };
            let refresh = |s: &mut Settings| {
                s.ensure_pairings(PairingContext {
                    root: &root,
                    chain: ChainId::Regtest,
                    cube_id: "tx-fixture",
                    wallet: Some(wallet.clone()),
                    daemon: Some(service.daemon.clone()),
                    supported: true,
                    proposal_supported: false,
                    last_poll_at_startup: None,
                })
            };
            session::open_without_pin("tx-fixture");
            let mut settings = attach(true);
            for message in [
                Message::ClientName("transaction fixture".into()),
                Message::Wallet(true),
                Message::Transactions(true),
                Message::Pair,
            ] {
                settings.update(
                    message,
                    Some(wallet.clone()),
                    Some(service.daemon.clone()),
                    true,
                    "tx-fixture",
                );
            }
            refresh(&mut settings);
            let client = settings.pairing_store.as_ref().unwrap().clients()[0].clone();
            let opaque = match &client.scope {
                pairings::Scope::TransactionPreparation { opaque, .. } => opaque.clone(),
                _ => panic!("wrong scope"),
            };
            let endpoint = location.locator(&client.id);
            let mut request = input();
            request.wallet_id = opaque.clone();
            let response = wire::ipc::call(
                &wire::pairing::resolve(&endpoint).unwrap(),
                &client.credential,
                wire::Request::PrepareTransaction(request.clone()),
            )
            .await
            .unwrap();
            let summary = match response {
                wire::Response::Transaction(v) => v,
                _ => panic!("wrong response"),
            };
            let native = settings
                .prepared_transaction(&client.id, &summary.draft_id, &wallet)
                .unwrap();
            assert!(native
                .psbt
                .inputs
                .iter()
                .all(|i| i.partial_sigs.is_empty() && i.tap_key_sig.is_none()));
            let panel =
                crate::app::state::CreateSpendPanel::from_mcp_prepared(wallet.clone(), native, 1);
            assert!(
                !panel.keep_state(),
                "native Send navigation must not reuse an old MCP review as a new form"
            );
            let mut replacement = (*wallet).clone();
            replacement.pinned_at = Some(123);
            assert!(settings
                .prepared_transaction(&client.id, &summary.draft_id, &replacement)
                .is_none());
            session::close();
            assert!(wire::ipc::call(
                &wire::pairing::resolve(&endpoint).unwrap(),
                &client.credential,
                wire::Request::PrepareTransaction(request.clone())
            )
            .await
            .is_err());
            assert!(settings
                .prepared_transaction(&client.id, &summary.draft_id, &wallet)
                .is_none());
            settings.revoke();
            drop(settings);
            session::open_without_pin("tx-fixture");
            let mut restored = attach(false);
            refresh(&mut restored);
            let old = wire::Request::Transaction(DraftInput {
                wallet_id: opaque.clone(),
                draft_id: summary.draft_id.clone(),
            });
            assert!(
                matches!(
                    wire::ipc::call(
                        &wire::pairing::resolve(&endpoint).unwrap(),
                        &client.credential,
                        old
                    )
                    .await,
                    Err(wire::Error::AccessDenied)
                ),
                "old draft survived a new unlock"
            );
            let response = wire::ipc::call(
                &wire::pairing::resolve(&endpoint).unwrap(),
                &client.credential,
                wire::Request::PrepareTransaction(request),
            )
            .await
            .unwrap();
            assert!(
                matches!(response,wire::Response::Transaction(v)if v.draft_id!=summary.draft_id)
            );
            restored.update(
                Message::RequestRevokePair(client.id.clone()),
                Some(wallet.clone()),
                Some(service.daemon.clone()),
                true,
                "tx-fixture",
            );
            restored.update(
                Message::RevokePair(client.id.clone()),
                Some(wallet.clone()),
                Some(service.daemon.clone()),
                true,
                "tx-fixture",
            );
            assert!(wire::pairing::resolve(&endpoint).is_err());
            drop(restored);
            let mut after = attach(false);
            refresh(&mut after);
            assert!(after.pairing_store.as_ref().unwrap().clients().is_empty());
            assert!(after.paired.is_empty());
            drop(after);
            drop(client);
            drop(wallet);
            session::close();
            cleanup(root, service).await;
        });
    }

    #[test]
    fn output_floor_accepts_the_network_minimum() {
        use coincube_core::miniscript::bitcoin::{hashes::Hash, PubkeyHash, WPubkeyHash};
        let p2wpkh = ScriptBuf::new_p2wpkh(&WPubkeyHash::all_zeros());
        let network_dust = p2wpkh.minimal_non_dust().to_sat();
        assert_eq!(network_dust, 294);
        assert!(output_below_floor(network_dust - 1, p2wpkh.clone()));
        assert!(!output_below_floor(network_dust, p2wpkh));

        let p2pkh = ScriptBuf::new_p2pkh(&PubkeyHash::all_zeros());
        let legacy_dust = p2pkh.minimal_non_dust().to_sat();
        assert_eq!(legacy_dust, 546);
        assert!(output_below_floor(legacy_dust - 1, p2pkh.clone()));
        assert!(!output_below_floor(legacy_dust, p2pkh));
    }
}

#[cfg(test)]
#[path = "transactions_production_tests.rs"]
mod production_tests;

//! Canonical, bounded authority record for an imported local-key inventory.
//!
//! The record belongs in a device-only OS keystore item, never alongside the
//! manifest that it authenticates. This module defines the wire contract before
//! the repository transaction and platform adapter consume it.

use std::convert::TryInto;

use coincube_core::miniscript::bitcoin::hashes::{sha256, Hash};
use uuid::Uuid;

#[cfg(target_os = "macos")]
#[cfg_attr(test, allow(dead_code))]
mod apple;

const TAG: &[u8] = b"COINCUBE imported inventory anchor\0";
const DIGEST_TAG: &[u8] = b"COINCUBE imported inventory manifest v1\0";
const VERSION: u8 = 1;
const MAX_FIELD: usize = 512;
const MAX_ITEM: usize = 8_192;
const MAX_RETIRE: usize = 32;
#[cfg(all(not(test), target_os = "macos"))]
const SERVICE: &str = "io.coincube.desktop.imported-inventory-anchor.v1";
/// Service name used by Tenshu-era builds. [`load_bytes`] migrates an anchor
/// found only here; see `apple::migrate_legacy`.
#[cfg(all(not(test), target_os = "macos"))]
const LEGACY_SERVICE: &str = "io.coincube.tenshu.imported-inventory-anchor.v1";

#[cfg(all(target_os = "macos", debug_assertions, feature = "native-anchor-probe"))]
mod native_probe_service {
    use std::cell::RefCell;

    thread_local! {
        static SERVICE: RefCell<Option<String>> = const { RefCell::new(None) };
    }

    pub(super) fn current() -> Option<String> {
        SERVICE.with(|slot| slot.borrow().clone())
    }

    pub(super) fn with<T>(run_id: uuid::Uuid, action: impl FnOnce() -> T) -> T {
        let service = format!("io.coincube.desktop.native-imported-anchor-probe.{run_id}");
        SERVICE.with(|slot| {
            assert!(slot.borrow().is_none(), "nested native anchor probe");
            *slot.borrow_mut() = Some(service);
        });
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                SERVICE.with(|slot| *slot.borrow_mut() = None);
            }
        }
        let _reset = Reset;
        action()
    }
}

#[cfg(all(target_os = "macos", debug_assertions, feature = "native-anchor-probe"))]
pub(in crate::services) fn with_native_probe_service<T>(
    run_id: Uuid,
    action: impl FnOnce() -> T,
) -> T {
    native_probe_service::with(run_id, action)
}

#[cfg(all(target_os = "macos", debug_assertions, feature = "native-anchor-probe"))]
pub(in crate::services) use apple::ProbeFault as NativeProbeFault;

#[cfg(all(target_os = "macos", debug_assertions, feature = "native-anchor-probe"))]
pub(in crate::services) fn arm_native_probe_fault(fault: NativeProbeFault) {
    let synthetic = native_probe_service::current().expect("native probe service is not active");
    apple::arm_probe_fault(&synthetic, fault);
}

#[cfg(all(not(test), target_os = "macos"))]
fn service() -> String {
    #[cfg(all(debug_assertions, feature = "native-anchor-probe"))]
    if let Some(synthetic) = native_probe_service::current() {
        return synthetic;
    }
    SERVICE.to_owned()
}

/// Read the current anchor item, migrating a Tenshu-era one if that is all
/// there is. Synthetic probe services have no legacy counterpart.
#[cfg(all(not(test), target_os = "macos"))]
fn load_bytes(account: &str) -> Result<Option<Vec<u8>>, String> {
    let service = service();
    if let Some(bytes) = apple::load(&service, account)? {
        return Ok(Some(bytes));
    }
    if service != SERVICE {
        return Ok(None);
    }
    apple::migrate_legacy(SERVICE, LEGACY_SERVICE, account)
}

pub(super) const fn supported() -> bool {
    cfg!(any(test, target_os = "macos"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Scope {
    pub cube_id: String,
    pub chain: String,
    pub operation_id: Uuid,
    pub wallet_id: String,
    pub policy_hash: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum State {
    Committed {
        generation: u64,
        digest: [u8; 32],
        cleanup_txn: Option<Uuid>,
        retire_files: Vec<String>,
    },
    Pending {
        old_generation: u64,
        old_digest: [u8; 32],
        next_digest: [u8; 32],
        txn_id: Uuid,
        retire_files: Vec<String>,
    },
    Revoked {
        generation: u64,
        digest: [u8; 32],
        txn_id: Uuid,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Anchor {
    pub scope: Scope,
    pub state: State,
}

pub(super) fn digest(bytes: &[u8]) -> [u8; 32] {
    let mut tagged = Vec::with_capacity(DIGEST_TAG.len() + bytes.len());
    tagged.extend_from_slice(DIGEST_TAG);
    tagged.extend_from_slice(bytes);
    sha256::Hash::hash(&tagged).to_byte_array()
}

pub(super) fn account(scope: &Scope) -> Result<String, String> {
    if scope.operation_id.is_nil()
        || scope.cube_id.is_empty()
        || scope.cube_id.len() > MAX_FIELD
        || scope.chain.is_empty()
        || scope.chain.len() > MAX_FIELD
    {
        return Err("Invalid imported inventory anchor account.".into());
    }
    // Hex fields make the boundary unambiguous even if IDs contain separators.
    Ok(format!(
        "{}:{}:{}",
        hex::encode(scope.cube_id.as_bytes()),
        hex::encode(scope.chain.as_bytes()),
        scope.operation_id
    ))
}

pub(super) fn load(scope: &Scope) -> Result<Option<Anchor>, String> {
    let account = account(scope)?;
    #[cfg(test)]
    {
        let bytes = tests::store_load(&account)?;
        let anchor = bytes.map(|bytes| Anchor::decode(&bytes)).transpose()?;
        if anchor.as_ref().is_some_and(|anchor| &anchor.scope != scope) {
            return Err("Imported inventory anchor belongs to another wallet.".into());
        }
        Ok(anchor)
    }
    #[cfg(all(not(test), target_os = "macos"))]
    {
        let bytes = load_bytes(&account)?;
        let anchor = bytes.map(|bytes| Anchor::decode(&bytes)).transpose()?;
        if anchor.as_ref().is_some_and(|anchor| &anchor.scope != scope) {
            return Err("Imported inventory anchor belongs to another wallet.".into());
        }
        Ok(anchor)
    }
    #[cfg(all(not(test), not(target_os = "macos")))]
    {
        let _ = account;
        Err("Imported local-key anchor requires a verified device keystore adapter on this platform.".into())
    }
}

/// An interrupted first provision may precede the public attachment, so its
/// policy hash is unavailable. Check the device-only account keyed only by
/// Cube, chain and operation; absence is required before treating disk files
/// as an unanchored empty inventory. Never decode it as signer authority.
pub(in crate::services) fn absent_for_operation(
    cube_id: &str,
    chain: &str,
    operation_id: Uuid,
) -> Result<bool, String> {
    let account = account(&Scope {
        cube_id: cube_id.to_owned(),
        chain: chain.to_owned(),
        operation_id,
        wallet_id: String::new(),
        policy_hash: [0; 32],
    })?;
    #[cfg(test)]
    {
        Ok(tests::store_load(&account)?.is_none())
    }
    #[cfg(all(not(test), target_os = "macos"))]
    {
        Ok(load_bytes(&account)?.is_none())
    }
    #[cfg(all(not(test), not(target_os = "macos")))]
    {
        let _ = account;
        Err("Imported local-key anchor requires a verified device keystore adapter on this platform.".into())
    }
}

pub(super) fn create(anchor: &Anchor) -> Result<(), String> {
    let account = account(&anchor.scope)?;
    let value = anchor.encode()?;
    #[cfg(test)]
    {
        tests::store_create(&account, &value)?;
        Ok(())
    }
    #[cfg(all(not(test), target_os = "macos"))]
    {
        let add = apple::create(&service(), &account, &value);
        if load(&anchor.scope)?.as_ref() == Some(anchor) {
            return Ok(());
        }
        Err(add
            .err()
            .unwrap_or_else(|| "Imported inventory anchor create was not confirmed.".into()))
    }
    #[cfg(all(not(test), not(target_os = "macos")))]
    {
        let _ = (account, value);
        Err("Imported local-key admission is unavailable on this platform.".into())
    }
}

pub(super) fn update(old: &Anchor, next: &Anchor) -> Result<(), String> {
    if old.scope != next.scope {
        return Err("Imported inventory anchor scope changed.".into());
    }
    let legal = match (&old.state, &next.state) {
        (
            State::Committed {
                generation,
                digest,
                cleanup_txn: None,
                retire_files: prior,
            },
            State::Pending {
                old_generation,
                old_digest,
                ..
            },
        ) => prior.is_empty() && generation == old_generation && digest == old_digest,
        (
            State::Pending {
                old_generation,
                next_digest,
                txn_id,
                retire_files: expected,
                ..
            },
            State::Committed {
                generation,
                digest,
                cleanup_txn,
                retire_files,
            },
        ) => {
            old_generation.checked_add(1) == Some(*generation)
                && next_digest == digest
                && expected == retire_files
                && *cleanup_txn == Some(*txn_id)
        }
        (
            State::Committed {
                generation: old_generation,
                digest: old_digest,
                cleanup_txn: Some(_),
                retire_files: old_files,
            },
            State::Committed {
                generation,
                digest,
                cleanup_txn: None,
                retire_files,
            },
        ) => {
            old_generation == generation
                && old_digest == digest
                && old_files.len() <= MAX_RETIRE
                && retire_files.is_empty()
        }
        (
            State::Committed {
                generation, digest, ..
            },
            State::Revoked {
                generation: next_generation,
                digest: next_digest,
                ..
            },
        ) => generation == next_generation && digest == next_digest,
        (
            State::Pending {
                old_generation,
                old_digest,
                ..
            },
            State::Revoked {
                generation, digest, ..
            },
        ) => old_generation == generation && old_digest == digest,
        _ => false,
    };
    if !legal {
        return Err("Invalid imported inventory anchor transition.".into());
    }
    let account = account(&next.scope)?;
    let value = next.encode()?;
    #[cfg(test)]
    {
        tests::store_update(&account, &old.encode()?, &value)?;
        Ok(())
    }
    #[cfg(all(not(test), target_os = "macos"))]
    {
        // Compare immediately before updating under the stable cross-process
        // repository lock. A mismatched item is never overwritten.
        if load(&old.scope)?.as_ref() != Some(old) {
            return Err("Imported inventory anchor changed before update.".into());
        }
        apple::update(&service(), &account, &value)?;
        if load(&next.scope)?.as_ref() != Some(next) {
            return Err("Imported inventory anchor update was not confirmed.".into());
        }
        Ok(())
    }
    #[cfg(all(not(test), not(target_os = "macos")))]
    {
        let _ = (account, value);
        Err("Imported local-key admission is unavailable on this platform.".into())
    }
}

fn field(out: &mut Vec<u8>, value: &str) -> Result<(), String> {
    let bytes = value.as_bytes();
    if bytes.is_empty() || bytes.len() > MAX_FIELD || bytes.contains(&0) {
        return Err("Invalid imported inventory anchor identity field.".into());
    }
    out.extend_from_slice(&(bytes.len() as u16).to_be_bytes());
    out.extend_from_slice(bytes);
    Ok(())
}

fn valid_retire(files: &[String]) -> bool {
    files.len() <= MAX_RETIRE
        && files.windows(2).all(|pair| pair[0] < pair[1])
        && files.iter().all(|name| {
            // UUIDs contain hyphens, so the first separator is not the file
            // boundary. Require the exact 36+1+36-byte shape instead.
            let bytes = name.as_bytes();
            bytes.len() == 77
                && name.is_ascii()
                && bytes[36] == b'-'
                && Uuid::parse_str(&name[..36]).is_ok_and(|id| !id.is_nil())
                && Uuid::parse_str(&name[37..73]).is_ok_and(|id| !id.is_nil())
                && &name[73..] == ".key"
                && bytes.iter().all(|byte| !byte.is_ascii_uppercase())
        })
}

impl Anchor {
    pub fn encode(&self) -> Result<Vec<u8>, String> {
        if self.scope.operation_id.is_nil() {
            return Err("Invalid imported inventory operation identity.".into());
        }
        let mut out = Vec::with_capacity(256);
        out.extend_from_slice(TAG);
        out.push(VERSION);
        field(&mut out, &self.scope.cube_id)?;
        field(&mut out, &self.scope.chain)?;
        out.extend_from_slice(self.scope.operation_id.as_bytes());
        field(&mut out, &self.scope.wallet_id)?;
        out.extend_from_slice(&self.scope.policy_hash);
        match &self.state {
            State::Committed {
                generation,
                digest,
                cleanup_txn,
                retire_files,
            } => encode_committed(&mut out, generation, digest, cleanup_txn, retire_files)?,
            State::Pending {
                old_generation,
                old_digest,
                next_digest,
                txn_id,
                retire_files,
            } => {
                old_generation
                    .checked_add(1)
                    .ok_or("Imported inventory generation overflow.")?;
                if txn_id.is_nil() || !valid_retire(retire_files) {
                    return Err("Invalid imported inventory transaction.".into());
                }
                out.push(2);
                out.extend_from_slice(&old_generation.to_be_bytes());
                out.extend_from_slice(old_digest);
                out.extend_from_slice(next_digest);
                out.extend_from_slice(txn_id.as_bytes());
                out.push(retire_files.len() as u8);
                for name in retire_files {
                    field(&mut out, name)?;
                }
            }
            State::Revoked {
                generation,
                digest,
                txn_id,
            } => {
                if txn_id.is_nil() {
                    return Err("Invalid imported inventory revocation.".into());
                }
                out.push(3);
                out.extend_from_slice(&generation.to_be_bytes());
                out.extend_from_slice(digest);
                out.extend_from_slice(txn_id.as_bytes());
            }
        }
        if out.len() > MAX_ITEM {
            return Err("Imported inventory anchor is oversized.".into());
        }
        Ok(out)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_ITEM || !bytes.starts_with(TAG) {
            return Err("Invalid imported inventory anchor domain or size.".into());
        }
        let mut reader = Reader(&bytes[TAG.len()..]);
        if reader.take(1)? != [VERSION] {
            return Err("Unknown imported inventory anchor version.".into());
        }
        let scope = Scope {
            cube_id: reader.field()?,
            chain: reader.field()?,
            operation_id: reader.uuid()?,
            wallet_id: reader.field()?,
            policy_hash: reader.array()?,
        };
        let state = match reader.take(1)?[0] {
            1 => decode_committed(&mut reader)?,
            2 => {
                let old_generation = u64::from_be_bytes(reader.array()?);
                let old_digest = reader.array()?;
                let next_digest = reader.array()?;
                let txn_id = reader.uuid()?;
                let count = reader.take(1)?[0] as usize;
                if count > MAX_RETIRE {
                    return Err("Imported inventory retirement list is oversized.".into());
                }
                let mut retire_files = Vec::with_capacity(count);
                for _ in 0..count {
                    retire_files.push(reader.field()?);
                }
                State::Pending {
                    old_generation,
                    old_digest,
                    next_digest,
                    txn_id,
                    retire_files,
                }
            }
            3 => State::Revoked {
                generation: u64::from_be_bytes(reader.array()?),
                digest: reader.array()?,
                txn_id: reader.uuid()?,
            },
            _ => return Err("Invalid imported inventory anchor state.".into()),
        };
        if !reader.0.is_empty() {
            return Err("Imported inventory anchor has trailing bytes.".into());
        }
        let anchor = Self { scope, state };
        if anchor.encode()?.as_slice() != bytes {
            return Err("Noncanonical imported inventory anchor.".into());
        }
        Ok(anchor)
    }
}

fn encode_committed(
    out: &mut Vec<u8>,
    generation: &u64,
    digest: &[u8; 32],
    cleanup_txn: &Option<Uuid>,
    retire_files: &[String],
) -> Result<(), String> {
    if !valid_retire(retire_files) || (cleanup_txn.is_none() && !retire_files.is_empty()) {
        return Err("Invalid imported inventory cleanup receipt.".into());
    }
    out.push(1);
    out.extend_from_slice(&generation.to_be_bytes());
    out.extend_from_slice(digest);
    out.push(u8::from(cleanup_txn.is_some()));
    if let Some(id) = cleanup_txn {
        if id.is_nil() {
            return Err("Invalid imported inventory cleanup transaction.".into());
        }
        out.extend_from_slice(id.as_bytes());
    }
    out.push(retire_files.len() as u8);
    for name in retire_files {
        field(out, name)?;
    }
    Ok(())
}

fn decode_committed(reader: &mut Reader<'_>) -> Result<State, String> {
    let generation = u64::from_be_bytes(reader.array()?);
    let digest = reader.array()?;
    let cleanup_txn = match reader.take(1)?[0] {
        0 => None,
        1 => Some(reader.uuid()?),
        _ => return Err("Invalid imported inventory cleanup flag.".into()),
    };
    let count = reader.take(1)?[0] as usize;
    if count > MAX_RETIRE {
        return Err("Imported inventory cleanup list is oversized.".into());
    }
    let mut retire_files = Vec::with_capacity(count);
    for _ in 0..count {
        retire_files.push(reader.field()?);
    }
    Ok(State::Committed {
        generation,
        digest,
        cleanup_txn,
        retire_files,
    })
}

struct Reader<'a>(&'a [u8]);
impl Reader<'_> {
    fn take(&mut self, count: usize) -> Result<&[u8], String> {
        if self.0.len() < count {
            return Err("Truncated imported inventory anchor.".into());
        }
        let (head, tail) = self.0.split_at(count);
        self.0 = tail;
        Ok(head)
    }
    fn array<const N: usize>(&mut self) -> Result<[u8; N], String> {
        self.take(N)?
            .try_into()
            .map_err(|_| "Truncated imported inventory anchor.".into())
    }
    fn uuid(&mut self) -> Result<Uuid, String> {
        let id = Uuid::from_bytes(self.array()?);
        if id.is_nil() {
            return Err("Nil imported inventory transaction or operation.".into());
        }
        Ok(id)
    }
    fn field(&mut self) -> Result<String, String> {
        let count = u16::from_be_bytes(self.array()?) as usize;
        if count == 0 || count > MAX_FIELD {
            return Err("Invalid imported inventory field length.".into());
        }
        let bytes = self.take(count)?;
        let value = String::from_utf8(bytes.to_vec())
            .map_err(|_| "Invalid imported inventory UTF-8.".to_string())?;
        if value.as_bytes().contains(&0) {
            return Err("Invalid imported inventory identity.".into());
        }
        Ok(value)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::{
        collections::HashMap,
        sync::{Mutex, OnceLock},
    };

    fn store() -> &'static Mutex<HashMap<String, Vec<u8>>> {
        static STORE: OnceLock<Mutex<HashMap<String, Vec<u8>>>> = OnceLock::new();
        STORE.get_or_init(|| Mutex::new(HashMap::new()))
    }

    #[derive(Clone, Copy)]
    pub(crate) enum UpdateFault {
        BeforeWrite,
        BeforeWriteTwoReadErrors,
        AfterWrite,
        AfterWriteReadbackError,
        AfterWriteStaleOldRead,
    }

    fn update_faults() -> &'static Mutex<HashMap<String, UpdateFault>> {
        static FAULTS: OnceLock<Mutex<HashMap<String, UpdateFault>>> = OnceLock::new();
        FAULTS.get_or_init(|| Mutex::new(HashMap::new()))
    }

    pub(in crate::services) fn inject_update_fault(scope: &Scope, fault: UpdateFault) {
        update_faults()
            .lock()
            .unwrap()
            .insert(account(scope).unwrap(), fault);
    }

    fn load_faults() -> &'static Mutex<HashMap<String, u8>> {
        static FAULTS: OnceLock<Mutex<HashMap<String, u8>>> = OnceLock::new();
        FAULTS.get_or_init(|| Mutex::new(HashMap::new()))
    }

    fn stale_loads() -> &'static Mutex<HashMap<String, Vec<u8>>> {
        static STALE: OnceLock<Mutex<HashMap<String, Vec<u8>>>> = OnceLock::new();
        STALE.get_or_init(|| Mutex::new(HashMap::new()))
    }

    pub(crate) fn store_load(account: &str) -> Result<Option<Vec<u8>>, String> {
        if let Some(stale) = stale_loads()
            .lock()
            .map_err(|_| "Mock anchor stale reads are poisoned.")?
            .remove(account)
        {
            return Ok(Some(stale));
        }
        {
            let mut faults = load_faults()
                .lock()
                .map_err(|_| "Mock anchor load faults are poisoned.")?;
            if let Some(count) = faults.get_mut(account) {
                *count -= 1;
                if *count == 0 {
                    faults.remove(account);
                }
                return Err("Injected anchor readback failure.".into());
            }
        }
        Ok(store()
            .lock()
            .map_err(|_| "Mock anchor store is poisoned.")?
            .get(account)
            .cloned())
    }

    pub(crate) fn store_create(account: &str, value: &[u8]) -> Result<(), String> {
        let mut store = store()
            .lock()
            .map_err(|_| "Mock anchor store is poisoned.")?;
        if store.contains_key(account) {
            return Err("Mock anchor already exists.".into());
        }
        store.insert(account.into(), value.into());
        Ok(())
    }

    pub(crate) fn store_update(account: &str, old: &[u8], next: &[u8]) -> Result<(), String> {
        let mut store = store()
            .lock()
            .map_err(|_| "Mock anchor store is poisoned.")?;
        if store
            .get(account)
            .is_none_or(|value| value.as_slice() != old)
        {
            return Err("Mock anchor changed before update.".into());
        }
        let fault = update_faults()
            .lock()
            .map_err(|_| "Mock anchor faults are poisoned.")?
            .remove(account);
        if matches!(fault, Some(UpdateFault::BeforeWriteTwoReadErrors)) {
            load_faults()
                .lock()
                .map_err(|_| "Mock anchor load faults are poisoned.")?
                .insert(account.into(), 2);
        }
        if matches!(
            fault,
            Some(UpdateFault::BeforeWrite | UpdateFault::BeforeWriteTwoReadErrors)
        ) {
            return Err("Injected anchor update failure before write.".into());
        }
        store.insert(account.into(), next.into());
        if matches!(fault, Some(UpdateFault::AfterWriteStaleOldRead)) {
            stale_loads()
                .lock()
                .map_err(|_| "Mock anchor stale reads are poisoned.")?
                .insert(account.into(), old.into());
        }
        if matches!(fault, Some(UpdateFault::AfterWriteReadbackError)) {
            load_faults()
                .lock()
                .map_err(|_| "Mock anchor load faults are poisoned.")?
                .insert(account.into(), 1);
        }
        if matches!(
            fault,
            Some(
                UpdateFault::AfterWrite
                    | UpdateFault::AfterWriteReadbackError
                    | UpdateFault::AfterWriteStaleOldRead
            )
        ) {
            return Err("Injected anchor update failure after write.".into());
        }
        Ok(())
    }

    pub(crate) fn store_remove(account: &str) {
        store().lock().unwrap().remove(account);
    }

    fn scope() -> Scope {
        Scope {
            cube_id: "cube-a".into(),
            chain: "regtest".into(),
            operation_id: Uuid::new_v4(),
            wallet_id: "wallet-a".into(),
            policy_hash: [7; 32],
        }
    }

    #[test]
    fn codec_rejects_noncanonical_and_truncated_items() {
        let anchor = Anchor {
            scope: scope(),
            state: State::Committed {
                generation: 0,
                digest: digest(b"empty"),
                cleanup_txn: None,
                retire_files: vec![],
            },
        };
        let bytes = anchor.encode().unwrap();
        assert_eq!(Anchor::decode(&bytes).unwrap(), anchor);
        for length in 0..bytes.len() {
            assert!(Anchor::decode(&bytes[..length]).is_err());
        }
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert!(Anchor::decode(&trailing).is_err());
        let mut unknown = bytes.clone();
        unknown[TAG.len()] = 2;
        assert!(Anchor::decode(&unknown).is_err());
    }

    #[test]
    fn retire_filename_codec_requires_key_suffix() {
        let filename = format!("{}-{}.key", Uuid::new_v4(), Uuid::new_v4());
        let anchor = Anchor {
            scope: scope(),
            state: State::Pending {
                old_generation: 0,
                old_digest: [1; 32],
                next_digest: [2; 32],
                txn_id: Uuid::new_v4(),
                retire_files: vec![filename],
            },
        };
        let encoded = anchor.encode().unwrap();
        assert_eq!(Anchor::decode(&encoded).unwrap(), anchor);

        let mut wrong_suffix = anchor.clone();
        if let State::Pending { retire_files, .. } = &mut wrong_suffix.state {
            retire_files[0].replace_range(73.., ".bak");
        }
        assert!(wrong_suffix.encode().is_err());

        let mut forged = encoded;
        let suffix_start = forged.len() - 4;
        forged[suffix_start..].copy_from_slice(b".bak");
        assert!(Anchor::decode(&forged).is_err());
    }

    #[test]
    fn pending_and_revoked_are_distinct_authorities() {
        let scope = scope();
        let pending = Anchor {
            scope: scope.clone(),
            state: State::Pending {
                old_generation: 4,
                old_digest: [1; 32],
                next_digest: [2; 32],
                txn_id: Uuid::new_v4(),
                retire_files: vec![],
            },
        };
        let revoked = Anchor {
            scope,
            state: State::Revoked {
                generation: 5,
                digest: [2; 32],
                txn_id: Uuid::new_v4(),
            },
        };
        assert_eq!(Anchor::decode(&pending.encode().unwrap()).unwrap(), pending);
        assert_eq!(Anchor::decode(&revoked.encode().unwrap()).unwrap(), revoked);
        assert_ne!(pending.encode().unwrap(), revoked.encode().unwrap());
    }

    #[test]
    fn transitions_advance_once_and_revocation_is_terminal() {
        let scope = scope();
        let old = Anchor {
            scope: scope.clone(),
            state: State::Committed {
                generation: 4,
                digest: [1; 32],
                cleanup_txn: None,
                retire_files: vec![],
            },
        };
        create(&old).unwrap();
        let txn = Uuid::new_v4();
        let pending = Anchor {
            scope: scope.clone(),
            state: State::Pending {
                old_generation: 4,
                old_digest: [1; 32],
                next_digest: [2; 32],
                txn_id: txn,
                retire_files: vec![],
            },
        };
        update(&old, &pending).unwrap();
        assert!(update(&old, &pending).is_err());
        assert_eq!(load(&scope).unwrap(), Some(pending.clone()));

        let committed = Anchor {
            scope: scope.clone(),
            state: State::Committed {
                generation: 5,
                digest: [2; 32],
                cleanup_txn: Some(txn),
                retire_files: vec![],
            },
        };
        let rollback = Anchor {
            scope: scope.clone(),
            state: old.state.clone(),
        };
        assert!(update(&pending, &rollback).is_err());
        update(&pending, &committed).unwrap();
        assert!(update(&pending, &committed).is_err());

        let cleared = Anchor {
            scope: scope.clone(),
            state: State::Committed {
                generation: 5,
                digest: [2; 32],
                cleanup_txn: None,
                retire_files: vec![],
            },
        };
        update(&committed, &cleared).unwrap();
        assert!(update(&cleared, &old).is_err());
        let revoked = Anchor {
            scope: scope.clone(),
            state: State::Revoked {
                generation: 5,
                digest: [2; 32],
                txn_id: Uuid::new_v4(),
            },
        };
        update(&cleared, &revoked).unwrap();
        assert!(update(&revoked, &cleared).is_err());
        assert_eq!(load(&scope).unwrap(), Some(revoked));
    }
}

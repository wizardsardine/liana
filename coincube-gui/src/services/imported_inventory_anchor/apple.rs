//! Dedicated data-protection Keychain item for imported inventory freshness.
//!
//! Item updates are read back, but Apple does not document sudden-power-loss
//! durability of `SecItemUpdate`. The native signed-app restart and fault tests
//! are a release gate; this adapter makes no stronger durability claim.

use core_foundation::base::{CFType, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::data::CFData;
use core_foundation::dictionary::CFDictionary;
use core_foundation::string::{CFString, CFStringRef};
use security_framework_sys::access_control::kSecAttrAccessibleWhenUnlockedThisDeviceOnly;
use security_framework_sys::base::{errSecDuplicateItem, errSecItemNotFound};
use security_framework_sys::item::{
    kSecAttrAccessGroup, kSecAttrAccount, kSecAttrService, kSecClass, kSecClassGenericPassword,
    kSecReturnAttributes, kSecReturnData, kSecValueData,
};
use security_framework_sys::keychain_item::{
    SecItemAdd, SecItemCopyMatching, SecItemDelete, SecItemUpdate,
};

extern "C" {
    static kSecAttrAccessible: CFStringRef;
    static kSecAttrSynchronizable: CFStringRef;
    static kSecUseDataProtectionKeychain: CFStringRef;
}

const ACCESS_GROUP: &str = "8UVR249AD5.io.coincube.desktop";

/// Group that Tenshu-era builds (bundle ID `io.coincube.tenshu`) wrote anchors
/// to. `coincube.entitlements` keeps it listed second so [`migrate_legacy`] can
/// move those anchors into [`ACCESS_GROUP`].
const LEGACY_ACCESS_GROUP: &str = "8UVR249AD5.io.coincube.tenshu";

#[cfg(all(debug_assertions, feature = "native-anchor-probe"))]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::services) enum ProbeFault {
    UpdateQueryError,
    UpdateThenError,
    UpdateThenReadbackError,
}

#[cfg(all(debug_assertions, feature = "native-anchor-probe"))]
mod probe_fault {
    use super::ProbeFault;
    use std::cell::Cell;

    thread_local! {
        static UPDATE: Cell<Option<ProbeFault>> = const { Cell::new(None) };
        static READBACK: Cell<bool> = const { Cell::new(false) };
    }

    pub(super) fn arm(fault: ProbeFault) {
        UPDATE.with(|slot| {
            assert!(slot.get().is_none(), "native anchor fault already armed");
            slot.set(Some(fault));
        });
    }

    pub(super) fn take_update() -> Option<ProbeFault> {
        UPDATE.with(|slot| slot.take())
    }

    pub(super) fn arm_readback() {
        READBACK.with(|slot| slot.set(true));
    }

    pub(super) fn take_readback() -> bool {
        READBACK.with(|slot| slot.replace(false))
    }
}

#[cfg(all(debug_assertions, feature = "native-anchor-probe"))]
pub(in crate::services) fn arm_probe_fault(service: &str, fault: ProbeFault) {
    assert!(
        service.starts_with("io.coincube.desktop.native-imported-anchor-probe."),
        "fault injection requires a synthetic service"
    );
    probe_fault::arm(fault);
}

#[cfg(all(debug_assertions, feature = "native-anchor-probe"))]
fn malformed_class(query: &mut Vec<(CFString, CFType)>) {
    let class = cfstr(unsafe { kSecClass });
    query.retain(|(key, _)| key != &class);
    query.push((class, CFData::from_buffer(b"invalid-class").into_CFType()));
}

fn cfstr(value: CFStringRef) -> CFString {
    unsafe { CFString::wrap_under_get_rule(value) }
}

fn identity(service: &str, account: &str) -> Vec<(CFString, CFType)> {
    identity_in(service, account, ACCESS_GROUP)
}

fn identity_in(service: &str, account: &str, group: &str) -> Vec<(CFString, CFType)> {
    vec![
        (
            cfstr(unsafe { kSecUseDataProtectionKeychain }),
            CFBoolean::from(true).into_CFType(),
        ),
        (
            cfstr(unsafe { kSecClass }),
            cfstr(unsafe { kSecClassGenericPassword }).into_CFType(),
        ),
        (
            cfstr(unsafe { kSecAttrService }),
            CFString::new(service).into_CFType(),
        ),
        (
            cfstr(unsafe { kSecAttrAccount }),
            CFString::new(account).into_CFType(),
        ),
        (
            cfstr(unsafe { kSecAttrAccessGroup }),
            CFString::new(group).into_CFType(),
        ),
    ]
}

fn verify_attributes(service: &str, account: &str) -> Result<(), String> {
    verify_attributes_in(service, account, ACCESS_GROUP)
}

fn verify_attributes_in(service: &str, account: &str, group: &str) -> Result<(), String> {
    let mut query = identity_in(service, account, group);
    query.push((
        cfstr(unsafe { kSecReturnAttributes }),
        CFBoolean::from(true).into_CFType(),
    ));
    let params = CFDictionary::from_CFType_pairs(&query);
    let mut output: core_foundation_sys::base::CFTypeRef = std::ptr::null();
    let status = unsafe { SecItemCopyMatching(params.as_concrete_TypeRef(), &mut output) };
    if status != 0 || output.is_null() {
        return Err("Imported inventory anchor attributes are unavailable.".into());
    }
    let attrs: CFDictionary<CFString, CFType> =
        unsafe { CFDictionary::wrap_under_create_rule(output as _) };
    let string_attr = |key| {
        attrs
            .find(cfstr(key))
            .and_then(|value| value.downcast::<CFString>())
            .map(|value| value.to_string())
    };
    if string_attr(unsafe { kSecAttrService }).as_deref() != Some(service)
        || string_attr(unsafe { kSecAttrAccount }).as_deref() != Some(account)
        || string_attr(unsafe { kSecAttrAccessGroup }).as_deref() != Some(group)
        || string_attr(unsafe { kSecAttrAccessible }).as_deref()
            != Some(
                cfstr(unsafe { kSecAttrAccessibleWhenUnlockedThisDeviceOnly })
                    .to_string()
                    .as_str(),
            )
    {
        return Err("Imported inventory anchor identity or device-only attributes changed.".into());
    }
    let synchronizable = attrs
        .find(cfstr(unsafe { kSecAttrSynchronizable }))
        .and_then(|value| value.downcast::<CFBoolean>())
        .map(Into::<bool>::into)
        .unwrap_or(false);
    if synchronizable {
        return Err("Imported inventory anchor is synchronizable.".into());
    }
    Ok(())
}

pub(super) fn load(service: &str, account: &str) -> Result<Option<Vec<u8>>, String> {
    let mut query = identity(service, account);
    #[cfg(all(debug_assertions, feature = "native-anchor-probe"))]
    let injected_readback = service
        .starts_with("io.coincube.desktop.native-imported-anchor-probe.")
        && probe_fault::take_readback();
    #[cfg(all(debug_assertions, feature = "native-anchor-probe"))]
    if injected_readback {
        malformed_class(&mut query);
    }
    query.push((
        cfstr(unsafe { kSecReturnData }),
        CFBoolean::from(true).into_CFType(),
    ));
    let params = CFDictionary::from_CFType_pairs(&query);
    let mut output: core_foundation_sys::base::CFTypeRef = std::ptr::null();
    let status = unsafe { SecItemCopyMatching(params.as_concrete_TypeRef(), &mut output) };
    #[cfg(all(debug_assertions, feature = "native-anchor-probe"))]
    if injected_readback {
        assert_ne!(
            status, 0,
            "synthetic malformed readback query unexpectedly succeeded"
        );
        return Err(format!(
            "Synthetic imported anchor readback query failed (OSStatus {status})."
        ));
    }
    if status == errSecItemNotFound {
        return Ok(None);
    }
    if status != 0 || output.is_null() {
        return Err(format!(
            "Cannot read imported inventory anchor (OSStatus {status})."
        ));
    }
    let data = unsafe { CFData::wrap_under_create_rule(output as _) };
    verify_attributes(service, account)?;
    Ok(Some(data.bytes().to_vec()))
}

pub(super) fn create(service: &str, account: &str, value: &[u8]) -> Result<(), String> {
    let mut query = identity(service, account);
    query.extend([
        (
            cfstr(unsafe { kSecAttrAccessible }),
            cfstr(unsafe { kSecAttrAccessibleWhenUnlockedThisDeviceOnly }).into_CFType(),
        ),
        (
            cfstr(unsafe { kSecAttrSynchronizable }),
            CFBoolean::from(false).into_CFType(),
        ),
        (
            cfstr(unsafe { kSecValueData }),
            CFData::from_buffer(value).into_CFType(),
        ),
    ]);
    let params = CFDictionary::from_CFType_pairs(&query);
    let status = unsafe { SecItemAdd(params.as_concrete_TypeRef(), std::ptr::null_mut()) };
    if status == errSecDuplicateItem {
        return Err("Imported inventory anchor already exists.".into());
    }
    if status != 0 {
        return Err(format!(
            "Cannot create imported inventory anchor (OSStatus {status})."
        ));
    }
    verify_attributes(service, account)
}

pub(super) fn update(service: &str, account: &str, value: &[u8]) -> Result<(), String> {
    let identity = identity(service, account);
    #[cfg(all(debug_assertions, feature = "native-anchor-probe"))]
    let mut identity = identity;
    #[cfg(all(debug_assertions, feature = "native-anchor-probe"))]
    let injected = if service.starts_with("io.coincube.desktop.native-imported-anchor-probe.") {
        probe_fault::take_update()
    } else {
        None
    };
    #[cfg(all(debug_assertions, feature = "native-anchor-probe"))]
    if injected == Some(ProbeFault::UpdateQueryError) {
        malformed_class(&mut identity);
    }
    let query = CFDictionary::from_CFType_pairs(&identity);
    let attributes = CFDictionary::from_CFType_pairs(&[(
        cfstr(unsafe { kSecValueData }),
        CFData::from_buffer(value).into_CFType(),
    )]);
    let status = unsafe {
        SecItemUpdate(
            query.as_concrete_TypeRef(),
            attributes.as_concrete_TypeRef(),
        )
    };
    #[cfg(all(debug_assertions, feature = "native-anchor-probe"))]
    if injected == Some(ProbeFault::UpdateQueryError) {
        assert_ne!(
            status, 0,
            "synthetic malformed update query unexpectedly succeeded"
        );
        return Err(format!(
            "Synthetic imported anchor update query failed (OSStatus {status})."
        ));
    }
    if status != 0 {
        return Err(format!(
            "Cannot update imported inventory anchor (OSStatus {status})."
        ));
    }
    #[cfg(all(debug_assertions, feature = "native-anchor-probe"))]
    if injected == Some(ProbeFault::UpdateThenReadbackError) {
        probe_fault::arm_readback();
        return Err("Synthetic imported anchor update completed before readback failure.".into());
    }
    #[cfg(all(debug_assertions, feature = "native-anchor-probe"))]
    if injected == Some(ProbeFault::UpdateThenError) {
        return Err("Synthetic imported anchor update completed before reported error.".into());
    }
    verify_attributes(service, account)
}

/// Move a Tenshu-era anchor into the current service and access group.
///
/// Called only after [`load`] found no current item. Returns `Ok(None)` when
/// there is no legacy item either. Otherwise the legacy item's attributes are
/// verified, its bytes are written as the current item (or an identical
/// current item from a concurrent migration is accepted), and the legacy item
/// is deleted. The bytes are returned for the caller to decode as usual.
///
/// The legacy item must not outlive the move. This is freshness state: a stale
/// copy that a later fallback could read back would be a rollback. So every
/// failure is an error rather than a silent fallback, and a failed delete is
/// reported even though the current item already exists (the next load then
/// finds the current item and never consults the legacy one).
pub(super) fn migrate_legacy(
    service: &str,
    legacy_service: &str,
    account: &str,
) -> Result<Option<Vec<u8>>, String> {
    let mut query = identity_in(legacy_service, account, LEGACY_ACCESS_GROUP);
    query.push((
        cfstr(unsafe { kSecReturnData }),
        CFBoolean::from(true).into_CFType(),
    ));
    let params = CFDictionary::from_CFType_pairs(&query);
    let mut output: core_foundation_sys::base::CFTypeRef = std::ptr::null();
    let status = unsafe { SecItemCopyMatching(params.as_concrete_TypeRef(), &mut output) };
    if status == errSecItemNotFound {
        return Ok(None);
    }
    if status != 0 || output.is_null() {
        return Err(format!(
            "Cannot read legacy imported inventory anchor (OSStatus {status})."
        ));
    }
    let legacy = unsafe { CFData::wrap_under_create_rule(output as _) }
        .bytes()
        .to_vec();
    verify_attributes_in(legacy_service, account, LEGACY_ACCESS_GROUP)?;

    if let Err(create_error) = create(service, account, &legacy) {
        // A concurrent migration may have written the same bytes first.
        match load(service, account)? {
            Some(current) if current == legacy => {}
            Some(_) => {
                return Err(
                    "Imported inventory anchor differs from its legacy copy; refusing to migrate."
                        .into(),
                )
            }
            None => return Err(create_error),
        }
    }
    if load(service, account)?.as_deref() != Some(legacy.as_slice()) {
        return Err("Migrated imported inventory anchor was not confirmed.".into());
    }

    let params =
        CFDictionary::from_CFType_pairs(&identity_in(legacy_service, account, LEGACY_ACCESS_GROUP));
    let status = unsafe { SecItemDelete(params.as_concrete_TypeRef()) };
    if status != 0 && status != errSecItemNotFound {
        return Err(format!(
            "Imported inventory anchor was migrated but its legacy copy could not be removed (OSStatus {status})."
        ));
    }
    Ok(Some(legacy))
}

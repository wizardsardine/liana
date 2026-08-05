//! macOS keychain item for the device secret, written through `SecItem`
//! directly rather than through `keyring`.
//!
//! # Why not `keyring`
//!
//! The plan requires the item to be `kSecAttrAccessibleWhenUnlockedThisDeviceOnly`
//! **and** non-synchronizable, and says to check rather than assume. Checked:
//! `keyring` 3.3.0's macOS backend goes through
//! `SecKeychain::set_generic_password` — the legacy *file* keychain API — and
//! sets **neither** attribute. There is no way to pass them through it.
//!
//! That matters more than a missing hardening flag. If the item lands in the
//! iCloud-synchronizable class, the second factor that makes a copied datadir
//! useless is uploaded to Apple and the entire Tier 1 design is defeated —
//! quietly, with everything appearing to work.
//!
//! So this one item uses `security-framework`/`security-framework-sys`
//! directly. The value keys we need (`kSecAttrAccessible`,
//! `kSecAttrSynchronizable`) are not re-exported by the sys crate, so they are
//! declared here against `Security.framework`, which that crate already links.
//!
//! # Verification, and its limit
//!
//! [`add_if_absent`] sets both attributes and [`verify_attributes`] reads them back
//! from the stored item, so a silently-ignored attribute is caught locally and
//! is covered by a test that runs on any Mac.
//!
//! What that cannot prove is the *behaviour*: that the item genuinely never
//! appears on a second Mac signed into the same Apple ID. That is the
//! acceptance check the device-bound decision calls for, it needs two machines,
//! and it has **not** been performed. See the module docs on
//! [`super`] for the full unverified list.

use core_foundation::base::{CFType, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::data::CFData;
use core_foundation::dictionary::CFDictionary;
use core_foundation::string::{CFString, CFStringRef};
use security_framework_sys::access_control::kSecAttrAccessibleWhenUnlockedThisDeviceOnly;
use security_framework_sys::base::errSecItemNotFound;
use security_framework_sys::item::{
    kSecAttrAccount, kSecAttrService, kSecClass, kSecClassGenericPassword, kSecReturnAttributes,
    kSecReturnData, kSecValueData,
};
use security_framework_sys::keychain_item::{SecItemAdd, SecItemCopyMatching, SecItemDelete};
use zeroize::Zeroizing;

use super::UnlockError;

// Not re-exported by `security-framework-sys` 2.10, but present in
// Security.framework, which that crate links. These are the *keys*; the sys
// crate only exports the accessibility *values*.
// `kSecUseDataProtectionKeychain` *is* in the sys crate but sits behind its
// `OSX_10_15` feature; declared here with the other two rather than turning on
// a feature flag whose other effects we would then own.
extern "C" {
    static kSecAttrAccessible: CFStringRef;
    static kSecAttrSynchronizable: CFStringRef;
    static kSecUseDataProtectionKeychain: CFStringRef;
}

fn cfstr(r: CFStringRef) -> CFString {
    unsafe { CFString::wrap_under_get_rule(r) }
}

/// The attribute pairs that identify this Cube's item.
///
/// `kSecUseDataProtectionKeychain` is on **every** query, not just the writes.
/// Without it macOS routes to the legacy *file* keychain, which silently
/// ignores `kSecAttrAccessible` — an item written there reports the attribute
/// as unset, which is how a "device-only" secret quietly becomes an ordinary
/// one. (Observed directly: `verify_attributes` returned `<unset>` until this
/// was added.) It must also be present on reads, or the lookup searches a
/// different keychain than the one that was written.
fn identity(service: &str, account: &str) -> Vec<(CFString, CFType)> {
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
    ]
}

/// The two attributes the design depends on, set explicitly on every write.
///
/// - `kSecAttrAccessible = WhenUnlockedThisDeviceOnly` — the item is not
///   readable while the machine is locked, and `ThisDeviceOnly` keeps it out of
///   keychain *backups* as well as out of sync.
/// - `kSecAttrSynchronizable = false` — never offered to iCloud Keychain.
fn required_attributes() -> Vec<(CFString, CFType)> {
    vec![
        (
            cfstr(unsafe { kSecAttrAccessible }),
            cfstr(unsafe { kSecAttrAccessibleWhenUnlockedThisDeviceOnly }).into_CFType(),
        ),
        (
            cfstr(unsafe { kSecAttrSynchronizable }),
            CFBoolean::from(false).into_CFType(),
        ),
    ]
}

/// Add the item. Returns `Ok(false)` when one already exists (`errSecDuplicateItem`),
/// which is what makes this usable as a create-if-absent primitive.
pub fn add_if_absent(service: &str, account: &str, secret: &[u8]) -> Result<bool, UnlockError> {
    let mut pairs = identity(service, account);
    pairs.extend(required_attributes());
    pairs.push((
        cfstr(unsafe { kSecValueData }),
        CFData::from_buffer(secret).into_CFType(),
    ));

    let params = CFDictionary::from_CFType_pairs(&pairs);
    let status = unsafe { SecItemAdd(params.as_concrete_TypeRef(), std::ptr::null_mut()) };

    const ERR_SEC_DUPLICATE_ITEM: i32 = security_framework_sys::base::errSecDuplicateItem;
    match status {
        0 => Ok(true),
        ERR_SEC_DUPLICATE_ITEM => Ok(false),
        other => Err(keychain_error(
            other,
            "your system keychain refused to store this Cube's key",
        )),
    }
}

/// Turn an `OSStatus` into the right message.
///
/// Shared by every entry point, because the entitlement failure is not specific
/// to writing. It was only mapped on `add_if_absent`, so an unsigned build hit
/// it at *unlock* — where nothing writes — and showed
/// "your system keychain couldn't be read (OSStatus -34018)" instead of the
/// message that explains the build needs signing. The raw number is the least
/// actionable thing we could have printed, at the moment the user most needs to
/// know what is wrong.
fn keychain_error(status: i32, context: &str) -> UnlockError {
    if status == ERR_SEC_MISSING_ENTITLEMENT {
        // `Unusable`, not `Unreachable`: the keychain is reachable and probably
        // already unlocked. What's missing is in the *binary*, so the retry
        // advice `KeystoreUnreachable` appends would send the user to unlock
        // something that isn't locked and never mention code signing.
        UnlockError::KeystoreUnusable(MISSING_ENTITLEMENT_MSG.to_string())
    } else {
        UnlockError::KeystoreUnreachable(format!("{context} (OSStatus {status})"))
    }
}

/// `errSecMissingEntitlement` — the data-protection keychain refused because
/// this binary is not code-signed with a `keychain-access-groups` entitlement.
pub const ERR_SEC_MISSING_ENTITLEMENT: i32 = -34018;

/// What that means, in the only terms that matter here.
///
/// **This is a shipping requirement, not a runtime hiccup.** The device secret
/// is only device-bound if it lives in the data-protection keychain, and that
/// keychain is only reachable from a signed binary. The legacy file keychain
/// *would* accept the item — and silently drop `kSecAttrAccessible`, leaving a
/// secret that is neither device-only nor guaranteed off iCloud while every
/// screen claims otherwise.
///
/// So this is surfaced as unavailable rather than retried against the legacy
/// keychain. Cube creation then refuses, exactly as it does on a headless Linux
/// box with no Secret Service, and for the same reason: a user who believes
/// they have a second factor and does not is worse off than one who was told
/// the truth.
pub const MISSING_ENTITLEMENT_MSG: &str =
    "This build of Coincube can't use the macOS data-protection keychain, so it \
     can't store part of your Cube's encryption key in a way that stays on this \
     Mac. This needs a signed build with the keychain-access-groups entitlement. \
     Cubes can't be created safely here.";

/// Read the item's data. `Ok(None)` when it does not exist.
pub fn load(service: &str, account: &str) -> Result<Option<Zeroizing<Vec<u8>>>, UnlockError> {
    let mut pairs = identity(service, account);
    pairs.push((
        cfstr(unsafe { kSecReturnData }),
        CFBoolean::from(true).into_CFType(),
    ));
    let params = CFDictionary::from_CFType_pairs(&pairs);

    let mut out: core_foundation_sys::base::CFTypeRef = std::ptr::null();
    let status = unsafe { SecItemCopyMatching(params.as_concrete_TypeRef(), &mut out) };
    if status == errSecItemNotFound {
        return Ok(None);
    }
    if status != 0 {
        return Err(keychain_error(
            status,
            "your system keychain couldn't be read",
        ));
    }
    if out.is_null() {
        return Ok(None);
    }
    // Wrap immediately so the bytes are scrubbed on every path out of here.
    let data = unsafe { CFData::wrap_under_create_rule(out as _) };
    Ok(Some(Zeroizing::new(data.bytes().to_vec())))
}

pub fn delete(service: &str, account: &str) -> Result<(), UnlockError> {
    let params = CFDictionary::from_CFType_pairs(&identity(service, account));
    let status = unsafe { SecItemDelete(params.as_concrete_TypeRef()) };
    if status == 0 || status == errSecItemNotFound {
        Ok(())
    } else {
        Err(keychain_error(
            status,
            "your system keychain couldn't remove this Cube's key",
        ))
    }
}

/// Read the stored item's attributes back and confirm both required ones took.
///
/// Setting an attribute the platform silently drops is exactly the failure this
/// design cannot survive, so the write is not trusted — it is checked.
pub fn verify_attributes(service: &str, account: &str) -> Result<(), UnlockError> {
    let mut pairs = identity(service, account);
    pairs.push((
        cfstr(unsafe { kSecReturnAttributes }),
        CFBoolean::from(true).into_CFType(),
    ));
    let params = CFDictionary::from_CFType_pairs(&pairs);

    let mut out: core_foundation_sys::base::CFTypeRef = std::ptr::null();
    let status = unsafe { SecItemCopyMatching(params.as_concrete_TypeRef(), &mut out) };
    if status != 0 || out.is_null() {
        return Err(keychain_error(
            status,
            "couldn't read back this Cube's keychain attributes",
        ));
    }
    let attrs: CFDictionary<CFString, CFType> =
        unsafe { CFDictionary::wrap_under_create_rule(out as _) };

    let accessible = attrs
        .find(cfstr(unsafe { kSecAttrAccessible }))
        .and_then(|v| v.downcast::<CFString>())
        .map(|s| s.to_string());
    let expected = cfstr(unsafe { kSecAttrAccessibleWhenUnlockedThisDeviceOnly }).to_string();
    if accessible.as_deref() != Some(expected.as_str()) {
        return Err(UnlockError::KeystoreUnreachable(format!(
            "this Cube's keychain item is not device-only (accessibility is {}, expected {})",
            accessible.as_deref().unwrap_or("<unset>"),
            expected
        )));
    }

    // Absent means false: a non-synchronizable item simply carries no
    // `kSecAttrSynchronizable`. Present-and-true is the failure.
    let synchronizable = attrs
        .find(cfstr(unsafe { kSecAttrSynchronizable }))
        .and_then(|v| v.downcast::<CFBoolean>())
        .map(Into::<bool>::into)
        .unwrap_or(false);
    if synchronizable {
        return Err(UnlockError::KeystoreUnreachable(
            "this Cube's keychain item is marked for iCloud sync, which would upload \
             part of its encryption key"
                .to_string(),
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// End-to-end against the real data-protection keychain.
    ///
    /// This is the check `keyring` made impossible: write the item, read the
    /// attributes back off the *stored* item, and confirm both took. It already
    /// earned its keep — it caught that a plain `SecItemAdd` lands in the legacy
    /// file keychain, which reports `kSecAttrAccessible` as unset.
    ///
    /// # Requires a signed binary
    ///
    /// `#[ignore]` because the data-protection keychain returns
    /// `errSecMissingEntitlement` (-34018) to any binary without a
    /// `keychain-access-groups` entitlement, and `cargo test` binaries are
    /// unsigned. This is not a test artefact: it is the same constraint the
    /// shipped app is under, which is why the production path treats it as
    /// "keystore unavailable" and refuses to create a Cube.
    ///
    /// Run against a signed build:
    /// ```text
    /// cargo test -p coincube-gui --lib device_secret -- --ignored
    /// ```
    ///
    /// Even then it does **not** prove the item never reaches a second Mac —
    /// that needs two machines and remains unverified.
    /// The entitlement failure must produce the actionable message from **every**
    /// entry point, not just writes.
    ///
    /// This is testable unsigned — indeed it is the case an unsigned build hits.
    #[test]
    fn missing_entitlement_is_explained_everywhere() {
        for context in [
            "your system keychain couldn't be read",
            "your system keychain couldn't remove this Cube's key",
            "couldn't read back this Cube's keychain attributes",
        ] {
            let e = keychain_error(ERR_SEC_MISSING_ENTITLEMENT, context);
            assert!(
                matches!(e, UnlockError::KeystoreUnusable(_)),
                "the entitlement failure is terminal for this build, not a \
                 reachability problem the user can clear"
            );
            let msg = e.to_string();
            assert!(
                msg.contains("signed build"),
                "an unsigned build gets a bare OSStatus instead of the reason: {}",
                msg
            );
            assert!(
                !msg.contains("-34018"),
                "the raw status is the least actionable thing to show: {}",
                msg
            );
            // The keychain is reachable and almost certainly already unlocked;
            // what is missing is in the binary. Retry advice here costs the
            // reader the one sentence that names the real cause.
            assert!(
                !msg.contains("try again"),
                "sends the user to unlock a keychain that isn't locked: {}",
                msg
            );
        }

        // Other statuses keep their context, the number, and the retry advice —
        // those genuinely are the transient case.
        let other = keychain_error(-25300, "your system keychain couldn't be read");
        assert!(matches!(other, UnlockError::KeystoreUnreachable(_)));
        let other = other.to_string();
        assert!(other.contains("-25300"));
        assert!(!other.contains("signed build"));
        assert!(other.contains("try again"));
    }

    #[test]
    #[ignore = "needs a code-signed binary (data-protection keychain returns -34018 unsigned)"]
    fn the_item_is_device_only_and_not_synchronizable() {
        const SERVICE: &str = "io.coincube.tenshu.test.device-secret";
        let account = format!("attr-probe-{}", std::process::id());

        let _ = delete(SERVICE, &account);
        let secret = [0xA5u8; 32];
        assert!(add_if_absent(SERVICE, &account, &secret).unwrap());

        verify_attributes(SERVICE, &account).expect("both attributes must be set on the item");

        let got = load(SERVICE, &account).unwrap().expect("item is readable");
        assert_eq!(got.as_slice(), &secret);

        // create-if-absent: a second add must not replace the first.
        assert!(!add_if_absent(SERVICE, &account, &[0x5Au8; 32]).unwrap());
        let still = load(SERVICE, &account).unwrap().unwrap();
        assert_eq!(
            still.as_slice(),
            &secret,
            "a duplicate add overwrote the winning value"
        );

        delete(SERVICE, &account).unwrap();
        assert!(load(SERVICE, &account).unwrap().is_none());
    }
}

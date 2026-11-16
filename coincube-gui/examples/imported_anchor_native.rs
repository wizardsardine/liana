//! Signed, disposable macOS probe for native imported-inventory transactions.
//! Every phase is a fresh process and uses a per-run synthetic Keychain service.

#[cfg(all(target_os = "macos", debug_assertions, feature = "native-anchor-probe"))]
fn main() {
    use std::path::PathBuf;
    use uuid::Uuid;

    let mut args = std::env::args().skip(1);
    let phase = args.next().expect("phase is required");
    let run_id = Uuid::parse_str(&args.next().expect("run UUID is required")).unwrap();
    assert!(!run_id.is_nil(), "nil run UUID is forbidden");
    let root = PathBuf::from(args.next().expect("disposable repository root is required"));
    assert!(args.next().is_none(), "unexpected argument");
    if phase == "delete" {
        delete(run_id);
    } else {
        coincube_gui::services::imported_keys::native_probe_phase(&phase, run_id, &root)
            .unwrap_or_else(|error| panic!("native imported anchor {}: {}", phase, error));
    }
    println!("imported anchor native phase {phase}: ok");
}

#[cfg(all(target_os = "macos", debug_assertions, feature = "native-anchor-probe"))]
fn delete(run_id: uuid::Uuid) {
    use core_foundation::{base::TCFType, dictionary::CFDictionary, string::CFString};
    use security_framework_sys::{
        base::errSecItemNotFound,
        item::{
            kSecAttrAccessGroup, kSecAttrAccount, kSecAttrService, kSecClass,
            kSecClassGenericPassword, kSecReturnData,
        },
        keychain_item::{SecItemCopyMatching, SecItemDelete},
    };
    extern "C" {
        static kSecUseDataProtectionKeychain: core_foundation::string::CFStringRef;
    }
    let key = |value| unsafe { CFString::wrap_under_get_rule(value) };
    let service = format!("io.coincube.desktop.native-imported-anchor-probe.{run_id}");
    let account = format!(
        "{}:{}:{}",
        hex::encode(format!("native-anchor-probe-{run_id}")),
        hex::encode("regtest"),
        run_id
    );
    let query = CFDictionary::from_CFType_pairs(&[
        (
            key(unsafe { kSecUseDataProtectionKeychain }),
            core_foundation::boolean::CFBoolean::from(true).into_CFType(),
        ),
        (
            key(unsafe { kSecClass }),
            key(unsafe { kSecClassGenericPassword }).into_CFType(),
        ),
        (
            key(unsafe { kSecAttrService }),
            CFString::new(&service).into_CFType(),
        ),
        (
            key(unsafe { kSecAttrAccount }),
            CFString::new(&account).into_CFType(),
        ),
        (
            key(unsafe { kSecAttrAccessGroup }),
            CFString::new("8UVR249AD5.io.coincube.desktop").into_CFType(),
        ),
    ]);
    let status = unsafe { SecItemDelete(query.as_concrete_TypeRef()) };
    assert!(
        status == 0 || status == errSecItemNotFound,
        "synthetic imported anchor Keychain deletion failed: {}",
        status
    );
    let verify = CFDictionary::from_CFType_pairs(&[
        (
            key(unsafe { kSecUseDataProtectionKeychain }),
            core_foundation::boolean::CFBoolean::from(true).into_CFType(),
        ),
        (
            key(unsafe { kSecClass }),
            key(unsafe { kSecClassGenericPassword }).into_CFType(),
        ),
        (
            key(unsafe { kSecAttrService }),
            CFString::new(&service).into_CFType(),
        ),
        (
            key(unsafe { kSecAttrAccount }),
            CFString::new(&account).into_CFType(),
        ),
        (
            key(unsafe { kSecAttrAccessGroup }),
            CFString::new("8UVR249AD5.io.coincube.desktop").into_CFType(),
        ),
        (
            key(unsafe { kSecReturnData }),
            core_foundation::boolean::CFBoolean::from(true).into_CFType(),
        ),
    ]);
    let mut output: core_foundation_sys::base::CFTypeRef = std::ptr::null();
    let remaining = unsafe { SecItemCopyMatching(verify.as_concrete_TypeRef(), &mut output) };
    assert_eq!(
        remaining, errSecItemNotFound,
        "synthetic Keychain item remained after deletion"
    );
}

#[cfg(not(all(target_os = "macos", debug_assertions, feature = "native-anchor-probe")))]
fn main() {
    panic!("native imported anchor probe requires a debug macOS build with native-anchor-probe");
}

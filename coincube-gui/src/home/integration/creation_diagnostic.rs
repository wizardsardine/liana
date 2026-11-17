//! Lossy, static diagnostics for the opt-in native driver only.
//!
//! Home's human-readable error stays private and unchanged. These source-owned
//! markers identify broad failure classes, not OS status or a recovery action.
//! Unrecognized messages deliberately keep the generic failure code.

pub(super) fn result<T>(
    results: impl IntoIterator<Item = Result<T, String>>,
) -> Result<T, &'static str> {
    let mut created = None;
    for result in results {
        // Preserve the original first-error refusal, even if later output exists.
        created = Some(result.map_err(|error| code(&error))?);
    }
    created.ok_or("cube_creation_result_missing")
}

const MISSING_ENTITLEMENT: &str =
    "This build of Coincube can't use the macOS data-protection keychain, so it \
    can't store part of your Cube's encryption key in a way that stays on this \
    Mac. This needs a signed build with the keychain-access-groups entitlement. \
    Cubes can't be created safely here.";
const KEYCHAIN_RETRY: &str = " Your Cube is safe — unlock your system keychain and try again.";

fn code(error: &str) -> &'static str {
    if error.starts_with("Couldn't read this Cube's files: couldn't open the provisioning lock:")
        || error
            .starts_with("Couldn't read this Cube's files: couldn't take the provisioning lock:")
    {
        "cube_device_secret_lock_failed"
    } else if error.starts_with("Failed to build master seed signer:") {
        "cube_master_signer_failed"
    } else if error.starts_with("Failed to create network directory:") {
        "cube_network_directory_failed"
    } else if error.starts_with("Failed to initialize GUI configuration:") {
        "cube_gui_configuration_failed"
    } else if error.starts_with("Failed to store master seed mnemonic:") {
        "cube_seed_storage_failed"
    } else if error == MISSING_ENTITLEMENT {
        "cube_keystore_missing_entitlement"
    } else if [
        "your system keychain refused to store this Cube's key",
        "your system keychain couldn't be read",
        "your system keychain couldn't remove this Cube's key",
        "couldn't read back this Cube's keychain attributes",
    ]
    .iter()
    .any(|context| error == format!("{context} (OSStatus -25308){KEYCHAIN_RETRY}"))
    {
        "cube_keystore_interaction_not_allowed"
    } else if error.ends_with(KEYCHAIN_RETRY)
        || error.starts_with("This build of Coincube can't use the macOS data-protection keychain,")
        || error
            .starts_with("Your system keychain accepted a test item and then reported it missing.")
        || error.starts_with("Your system keychain returned unexpected data.")
    {
        "cube_keystore_unavailable"
    } else {
        "cube_persistence_failed"
    }
}

#[cfg(test)]
mod tests {
    use super::{code, result, KEYCHAIN_RETRY, MISSING_ENTITLEMENT};

    #[test]
    fn identifies_only_fixed_macos_refusals() {
        assert_eq!(
            code(MISSING_ENTITLEMENT),
            "cube_keystore_missing_entitlement"
        );
        for context in [
            "your system keychain refused to store this Cube's key",
            "your system keychain couldn't be read",
            "your system keychain couldn't remove this Cube's key",
            "couldn't read back this Cube's keychain attributes",
        ] {
            assert_eq!(
                code(&format!("{context} (OSStatus -25308){KEYCHAIN_RETRY}")),
                "cube_keystore_interaction_not_allowed"
            );
            assert_eq!(
                code(&format!("{context} (OSStatus -25300){KEYCHAIN_RETRY}")),
                "cube_keystore_unavailable"
            );
        }
        assert_eq!(
            code("unrecognized context (OSStatus -25308)"),
            "cube_persistence_failed"
        );
        assert_eq!(
            code(&format!(
                "unrecognized context (OSStatus -25308){KEYCHAIN_RETRY}"
            )),
            "cube_keystore_unavailable"
        );
    }

    #[test]
    fn missing_output_and_persistence_refusal_are_distinct() {
        assert_eq!(result::<()>([]), Err("cube_creation_result_missing"));
        assert_eq!(
            result::<()>([Err("synthetic-private-detail".into())]),
            Err("cube_persistence_failed")
        );
        assert_eq!(result([Ok(7)]), Ok(7));
        assert_eq!(
            result([Err("synthetic-private-detail".into()), Ok(7)]),
            Err("cube_persistence_failed")
        );
    }

    #[test]
    fn classifies_only_known_creation_markers() {
        for (message, expected) in [
            (
                "Couldn't read this Cube's files: couldn't open the provisioning lock: detail",
                "cube_device_secret_lock_failed",
            ),
            (
                "Couldn't read this Cube's files: couldn't take the provisioning lock: detail",
                "cube_device_secret_lock_failed",
            ),
            (
                "Failed to build master seed signer: detail",
                "cube_master_signer_failed",
            ),
            (
                "Failed to create network directory: detail",
                "cube_network_directory_failed",
            ),
            (
                "Failed to initialize GUI configuration: detail",
                "cube_gui_configuration_failed",
            ),
            (
                "Failed to store master seed mnemonic: detail",
                "cube_seed_storage_failed",
            ),
            (
                "detail Your Cube is safe — unlock your system keychain and try again.",
                "cube_keystore_unavailable",
            ),
            (
                "This build of Coincube can't use the macOS data-protection keychain, detail",
                "cube_keystore_unavailable",
            ),
            (
                "Your system keychain accepted a test item and then reported it missing. detail",
                "cube_keystore_unavailable",
            ),
            (
                "Your system keychain returned unexpected data. detail",
                "cube_keystore_unavailable",
            ),
        ] {
            assert_eq!(code(message), expected);
        }
    }

    #[test]
    fn arbitrary_details_never_become_diagnostics() {
        let private_marker = "synthetic-private-detail";
        for prefix in [
            "",
            "Failed to store master seed mnemonic: ",
            "Failed to create network directory: ",
        ] {
            let message = format!("{prefix}{private_marker}");
            let diagnostic = code(&message);
            assert!(!diagnostic.contains(private_marker));
        }
        assert_eq!(code(private_marker), "cube_persistence_failed");
        assert_eq!(
            code("unrecognized keychain issue"),
            "cube_persistence_failed"
        );
    }
}

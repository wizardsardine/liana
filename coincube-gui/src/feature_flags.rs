//! Compile-time feature flags driven by the `.env` file (via `build.rs`).
//!
//! These are not Cargo features — they're string environment variables read
//! at build time via [`option_env!`] so they can be toggled per-build without
//! a recompile of dependencies.
//!
//! `build.rs` forwards every key from the project-root `.env` file through as
//! `cargo:rustc-env=KEY=VALUE`, so each key becomes visible to `option_env!`
//! during compilation of this crate.

/// Whether the passkey-based Cube creation flow is enabled.
///
/// Controlled by the `COINCUBE_ENABLE_PASSKEY` env var at build time.
/// Defaults to `false`. When `false`:
///
/// - The "Use Passkey" toggle is hidden from the Create Cube form.
/// - `Home::passkey_mode` is forced to `false` on init and after dismiss.
/// - The `CreateCube` handler's passkey branch becomes dead code.
/// - All passkey service code still compiles but is unreachable.
///
/// When `true`, the existing passkey code path re-activates (webview on
/// non-macOS, native AuthenticationServices on macOS).
pub const PASSKEY_ENABLED: bool = is_truthy(option_env!("COINCUBE_ENABLE_PASSKEY"));

/// Whether the cube-scoped Members UI (W8 / PLAN-cube-membership-desktop) is
/// shown in the Connect sidebar.
///
/// Controlled by the `COINCUBE_CUBE_MEMBERS_UI` env var at build time.
/// Defaults to `false` while the backend dependencies are rolling out. When
/// `false`, the `Members` sub-menu button is hidden from the sidebar — the
/// rest of the panel code still compiles but is unreachable via UI.
pub const CUBE_MEMBERS_UI_ENABLED: bool = is_truthy(option_env!("COINCUBE_CUBE_MEMBERS_UI"));

/// Whether the heir "Recover a Vault" discovery surface (COIN-377 / PR 1) is
/// shown on the launcher.
///
/// Controlled by the `COINCUBE_ENABLE_RECOVER_VAULT` env var at build time.
/// Defaults to `false`: the surface depends on the API's net-new
/// `/connect/cubes/recoverable` endpoint and the COIN-376 recovery sweep, so it
/// stays dark until both ship. When `false`, the "Recover a Vault" sidebar entry
/// is hidden; the panel code still compiles but is unreachable via UI.
pub const RECOVER_VAULT_ENABLED: bool = is_truthy(option_env!("COINCUBE_ENABLE_RECOVER_VAULT"));

/// Whether the **owner keychain recovery** surfaces (PLAN-owner-keychain-recovery)
/// are shown. This is the "protect with my phone" Recovery-Kit branch: the owner
/// mints an `owner-self` recovery key on their Keychain, seals their own seed /
/// descriptor to it (ECIES), and can later restore the Cube on a wiped install
/// by approving a Keychain decrypt — no recovery password.
///
/// Controlled by the `COINCUBE_OWNER_KEYCHAIN_RECOVERY` env var at build time.
/// Defaults to `true` now that the feature's dependencies have shipped: the
/// net-new `coincube-api` (`recovery-kit/recipients`, `recovery-kit/envelope`)
/// surfaces and the `keychain-app` owner-self key mint (COIN-390). This is the
/// one flag that defaults **on** — set `COINCUBE_OWNER_KEYCHAIN_RECOVERY=0`
/// (or `false`/`no`) at build time as a kill switch to dark-ship it again. When
/// disabled, the wizard's protection-mode picker hides the phone branches and
/// the "Recover a Cube I own" entry is hidden; the code still compiles but is
/// unreachable via UI.
pub const OWNER_KEYCHAIN_RECOVERY_ENABLED: bool =
    !is_falsy(option_env!("COINCUBE_OWNER_KEYCHAIN_RECOVERY"));

/// `const`-compatible truthy check: accepts `"1"`, `"true"`, `"yes"`
/// (case-insensitive for the latter two). Anything else, including `None`,
/// is `false`.
///
/// Uses byte-slice comparison because `str` equality is not yet stable in
/// `const` contexts on stable Rust.
const fn is_truthy(val: Option<&str>) -> bool {
    let Some(s) = val else {
        return false;
    };
    let b = s.as_bytes();
    bytes_eq_ci(b, b"1") || bytes_eq_ci(b, b"true") || bytes_eq_ci(b, b"yes")
}

/// `const`-compatible falsy check: accepts `"0"`, `"false"`, `"no"`
/// (case-insensitive for the latter two). Anything else, including `None`, is
/// `false` (i.e. "not explicitly disabled"). Used to make a flag default **on**
/// while keeping a build-time kill switch: `!is_falsy(option_env!(...))`.
const fn is_falsy(val: Option<&str>) -> bool {
    let Some(s) = val else {
        return false;
    };
    let b = s.as_bytes();
    bytes_eq_ci(b, b"0") || bytes_eq_ci(b, b"false") || bytes_eq_ci(b, b"no")
}

/// Case-insensitive byte-slice equality, const-stable.
const fn bytes_eq_ci(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut i = 0;
    while i < a.len() {
        let ac = a[i];
        let bc = b[i];
        // Lowercase ASCII letters (bit 0x20 set on A-Z)
        let al = if ac >= b'A' && ac <= b'Z' {
            ac | 0x20
        } else {
            ac
        };
        let bl = if bc >= b'A' && bc <= b'Z' {
            bc | 0x20
        } else {
            bc
        };
        if al != bl {
            return false;
        }
        i += 1;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_truthy_accepts_known_values() {
        assert!(is_truthy(Some("1")));
        assert!(is_truthy(Some("true")));
        assert!(is_truthy(Some("TRUE")));
        assert!(is_truthy(Some("True")));
        assert!(is_truthy(Some("yes")));
        assert!(is_truthy(Some("YES")));
    }

    #[test]
    fn is_truthy_rejects_unknown_values() {
        assert!(!is_truthy(None));
        assert!(!is_truthy(Some("")));
        assert!(!is_truthy(Some("0")));
        assert!(!is_truthy(Some("false")));
        assert!(!is_truthy(Some("no")));
        assert!(!is_truthy(Some("on")));
        assert!(!is_truthy(Some("off")));
        assert!(!is_truthy(Some("2")));
    }

    #[test]
    fn is_falsy_accepts_disabling_values() {
        assert!(is_falsy(Some("0")));
        assert!(is_falsy(Some("false")));
        assert!(is_falsy(Some("FALSE")));
        assert!(is_falsy(Some("False")));
        assert!(is_falsy(Some("no")));
        assert!(is_falsy(Some("NO")));
    }

    #[test]
    fn is_falsy_rejects_everything_else() {
        // `None` / unset must NOT be falsy — that's what makes the flag
        // default-on. Unknown strings likewise leave the feature enabled.
        assert!(!is_falsy(None));
        assert!(!is_falsy(Some("")));
        assert!(!is_falsy(Some("1")));
        assert!(!is_falsy(Some("true")));
        assert!(!is_falsy(Some("yes")));
        assert!(!is_falsy(Some("on")));
        assert!(!is_falsy(Some("off")));
        assert!(!is_falsy(Some("2")));
    }
}

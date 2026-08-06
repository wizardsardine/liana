//! Compile-time feature flags driven by the `.env` file (via `build.rs`).
//!
//! These are not Cargo features — they're string environment variables read
//! at build time via [`option_env!`] so they can be toggled per-build without
//! a recompile of dependencies.
//!
//! `build.rs` forwards every key from the project-root `.env` file through as
//! `cargo:rustc-env=KEY=VALUE`, so each key becomes visible to `option_env!`
//! during compilation of this crate.

/// Whether the passkey path — creating a Cube with one, and opening it — is
/// enabled at all.
///
/// Controlled by the `COINCUBE_ENABLE_PASSKEY` env var at build time. When
/// `false`:
///
/// - The Passkey/PIN toggle is hidden from the Create Cube form.
/// - `Home::passkey_mode` is `false` on init and after dismiss (it tracks
///   [`PASSKEY_CREATION_AVAILABLE`], so the PIN path is the only one).
/// - The `CreateCube` handler's passkey branch becomes dead code.
/// - `gui::tab` refuses to open an existing passkey Cube, naming the flag.
/// - All passkey service code still compiles but is unreachable.
///
/// # What this used to be, and why it changed
///
/// This flag carried a `DANGER` block for months. It said, correctly, that
/// enabling it could create a Cube that **cannot be opened**: passkey
/// registration worked and passkey unlock did not, so a Cube created with the
/// flag on was openable only if its owner had written the mnemonic down.
///
/// Every item on that list is now closed:
///
/// - **Unlock works.** `crate::passkey_unlock` runs the assertion, derives the
///   seed through HKDF + BIP39, and hands it to the Liquid and Spark loaders as
///   `SeedSource::InMemory` — a passkey Cube has no seed file, and the loaders
///   no longer assume one.
/// - **The Recovery Kit carries the seed** (**I11**). A re-authentication
///   ceremony at Kit time re-derives it, so recovery no longer depends on the
///   user having written twelve words down, or on iCloud Keychain being on.
/// - **The signing prerequisites are met**: `io.coincube.tenshu` is signed with
///   `keychain-access-groups` and `com.apple.developer.associated-domains`,
///   authorised by the embedded provisioning profile, and the AASA at
///   coincube.io lists the app under `webcredentials`.
/// - **`COINCUBE_PASSKEY_RP_ID`** is set to `coincube.io` in `.env`; the
///   compiled default stays `localhost` for dev builds.
///
/// # What is still true, and is now permanent
///
/// The credential is **synced**, not device-bound. Apple holds the credential
/// that deterministically produces a passkey Cube's master seed, and from the
/// first shipped passkey Cube onward that cannot be tightened without stranding
/// it — reversing the call was free only while no passkey Cube existed outside
/// a developer machine, and shipping spends that window. That is the trade
/// `company-brain/decisions/2026-08-04-tenshu-passkey-apple-id-bound.md` made
/// knowingly.
///
/// It also means the **acceptance check inverted**. An earlier decision made
/// the passkey device-bound and asked for proof the credential did *not*
/// travel; what must be proved now is the opposite — that it *does* reach a
/// second Mac on the same Apple ID and derives the same seed (compare xpubs,
/// never mnemonics) — and, separately and more importantly, that a Recovery Kit
/// alone restores the Cube on a machine with no access to that Apple ID.
/// Anyone working from the superseded plan will run the wrong test and read a
/// pass as a failure.
///
/// Device-binding is the PIN path's property, delivered by the `ENCRYPTED_V3`
/// device secret. The PIN path itself is unchanged, but it is no longer the
/// default: the Create Cube form presents Passkey/PIN as an A/B choice with
/// Passkey selected wherever [`PASSKEY_CREATION_AVAILABLE`] holds.
pub const PASSKEY_ENABLED: bool = is_truthy(option_env!("COINCUBE_ENABLE_PASSKEY"));

/// Whether the Create Cube form may offer a passkey on *this* platform.
///
/// macOS only, per
/// `company-brain/decisions/2026-08-03-passkey-macos-only-for-now.md`. The
/// unlock ceremony is `services::passkey::macos`; Windows and Linux have no
/// working equivalent (`windows.rs` still builds its own unverified origin
/// string, which is part of why it is deferred). Offering the toggle there
/// would create Cubes that this build cannot open — the exact failure the
/// flag's old `DANGER` note existed to prevent.
///
/// [`PASSKEY_ENABLED`] deliberately stays platform-agnostic: it also gates
/// *opening* a Cube, and a passkey Cube whose datadir is carried to Linux
/// should be told this OS can't open it, not that the feature is off.
pub const PASSKEY_CREATION_AVAILABLE: bool = PASSKEY_ENABLED && cfg!(target_os = "macos");

// A build with the passkey path on must know its Relying Party ID. If it does
// not, `services::passkey::RP_ID` falls back to the dev value `localhost`,
// which never matches the AASA at coincube.io: registration and assertion both
// fail, and nothing upstream of a running app reveals it — codesign,
// notarization and `spctl` all pass on a build with the wrong RP id.
//
// What this guards is a CI misconfiguration, not a typo. A GitHub Actions
// `${{ vars.X }}` on an undefined variable expands to the *empty string*, and
// the runner exports the variable anyway, so `option_env!` yields `Some("")`
// rather than `None`. Mapping the variable at the wrong scope — under an
// Environment rather than at repo level, where a job with no `environment:`
// key cannot see it — produces exactly that.
const _: () = assert!(
    !PASSKEY_ENABLED || is_set(option_env!("COINCUBE_PASSKEY_RP_ID")),
    "COINCUBE_ENABLE_PASSKEY is on but COINCUBE_PASSKEY_RP_ID is unset or empty"
);

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

/// `const`-compatible "present and non-empty" check for a build-time variable.
///
/// `option_env!` distinguishes unset (`None`) from set-but-empty (`Some("")`),
/// and for anything sourced from CI the second case is the common one: a GitHub
/// Actions `${{ vars.X }}` on an undefined variable expands to the empty string
/// and the runner exports the variable regardless. Treat that as "not
/// configured", because it is a misconfiguration rather than a value.
pub const fn is_set(val: Option<&str>) -> bool {
    match val {
        Some(s) => !s.is_empty(),
        None => false,
    }
}

/// The value of a build-time variable, or `default` when it is unset **or
/// empty**.
///
/// The string-valued counterpart to `is_truthy`. Prefer it over a bare
/// `match option_env!(..) { Some(v) => v, None => .. }`, which binds `Some("")`
/// to `v` and hands the empty string to callers instead of falling back — see
/// [`is_set`] for why CI produces that case routinely.
pub const fn non_empty_or(val: Option<&'static str>, default: &'static str) -> &'static str {
    match val {
        Some(s) => {
            if s.is_empty() {
                default
            } else {
                s
            }
        }
        None => default,
    }
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
    fn is_set_rejects_unset_and_empty() {
        // `Some("")` is what a GitHub Actions `${{ vars.X }}` on an undefined
        // variable produces. It must read as "not configured", not as a value.
        assert!(!is_set(None));
        assert!(!is_set(Some("")));
        assert!(is_set(Some("coincube.io")));
        assert!(is_set(Some(" ")));
    }

    #[test]
    fn non_empty_or_falls_back_on_unset_and_empty() {
        assert_eq!(non_empty_or(None, "localhost"), "localhost");
        assert_eq!(non_empty_or(Some(""), "localhost"), "localhost");
        assert_eq!(
            non_empty_or(Some("coincube.io"), "localhost"),
            "coincube.io"
        );
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

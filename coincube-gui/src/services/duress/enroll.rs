//! Duress enrollment — credential validation and secret generation (Phase 2).
//!
//! These are the pure, testable rules behind the enrollment wizard:
//!
//!   * the duress PIN just needs to be non-empty here — it must not collide
//!     with any Cube's real unlock PIN, but that's enforced in
//!     `persist_duress_enrollment` where the Cube PIN hashes are available,
//!   * the all-clear passphrase must be long enough to survive months of
//!     disuse and distinct from the duress PIN,
//!   * each desktop generates its **own** ~128-bit duress code with a CSPRNG and
//!     only ever sends the argon2id hash to the server.
//!
//! The UI layer renders entropy meters and step navigation on top of this; the
//! security-relevant decisions all live here where they can be unit-tested.

use argon2::{
    password_hash::{rand_core::OsRng as ArgonOsRng, PasswordHasher, SaltString},
    Argon2, Params,
};
use rand::RngCore;

/// Minimum all-clear passphrase length (characters).
pub const MIN_ALL_CLEAR_LEN: usize = 12;

/// Recommended all-clear passphrase length (characters), surfaced in the UI
/// entropy meter as the "strong" threshold.
pub const RECOMMENDED_ALL_CLEAR_LEN: usize = 24;

/// Argon2id parameters, matching the regular-PIN and recovery-kit KDFs
/// (19 MiB, 2 iterations, 1 lane) so duress secrets are no weaker than the
/// rest of the app. Test builds drop to the argon2 minimum: the suite hashes
/// secrets many times over and the 19 MiB cost starves CI under parallel load.
/// Verification reads the parameters from the stored PHC string, so a hash
/// made at either cost still round-trips.
#[cfg(not(test))]
const ARGON_M_COST: u32 = 19456;
#[cfg(not(test))]
const ARGON_T_COST: u32 = 2;
#[cfg(not(test))]
const ARGON_P_COST: u32 = 1;
#[cfg(test)]
const ARGON_M_COST: u32 = 8;
#[cfg(test)]
const ARGON_T_COST: u32 = 1;
#[cfg(test)]
const ARGON_P_COST: u32 = 1;

/// Bits of entropy in this desktop's generated duress code.
const DURESS_CODE_BITS: usize = 128;

/// The five unlock-delay choices offered as chips in the wizard. `H24` is the
/// default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DuressDelay {
    #[default]
    H24,
    H48,
    D7,
    D30,
    D90,
}

impl DuressDelay {
    /// All choices in display order; the first is the default (24h).
    pub const ALL: [DuressDelay; 5] = [
        DuressDelay::H24,
        DuressDelay::H48,
        DuressDelay::D7,
        DuressDelay::D30,
        DuressDelay::D90,
    ];

    /// Lockout window length in minutes, sent to Connect as
    /// `unlock_delay_minutes`.
    pub fn minutes(self) -> u32 {
        match self {
            DuressDelay::H24 => 24 * 60,
            DuressDelay::H48 => 48 * 60,
            DuressDelay::D7 => 7 * 24 * 60,
            DuressDelay::D30 => 30 * 24 * 60,
            DuressDelay::D90 => 90 * 24 * 60,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            DuressDelay::H24 => "24h",
            DuressDelay::H48 => "48h",
            DuressDelay::D7 => "7d",
            DuressDelay::D30 => "30d",
            DuressDelay::D90 => "90d",
        }
    }
}

/// Minimum edit distance between any two duress-related credentials.
///
/// Invariant I3. A duress PIN one keystroke away from the real one is worse
/// than no duress PIN at all: the user fat-fingers their own unlock and wipes
/// the device. `1234` / `1235` is the canonical case.
pub const MIN_CREDENTIAL_DISTANCE: usize = 2;

/// Levenshtein edit distance.
///
/// Two rows rather than a full matrix — the inputs here are a 4-digit PIN and a
/// passphrase, so this is not hot, but the full matrix buys nothing.
pub fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }

    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur[j + 1] = (prev[j + 1] + 1).min(cur[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// Whether `candidate` is too close to `existing` to be a safe duress credential.
///
/// Distance covers substitutions, insertions and deletions. Two extra patterns
/// the duress decision names explicitly are **not** caught by distance on short
/// strings and are checked separately:
///
/// - **reversed** — `1234` / `4321` is distance 4 but is the same secret to a
///   user recalling it under stress.
/// - **off-by-one digits** — `1234` / `2345`, every digit shifted. This is
///   distance **2**, so it sits exactly on the ≥2 boundary and the distance rule
///   alone would *accept* it. It is the "I'll just add one" pattern people
///   actually reach for, which is what makes the separate check load-bearing
///   rather than belt-and-braces.
pub fn too_similar(candidate: &str, existing: &str) -> bool {
    if candidate == existing {
        return true;
    }
    if levenshtein(candidate, existing) < MIN_CREDENTIAL_DISTANCE {
        return true;
    }
    if candidate.chars().rev().collect::<String>() == existing {
        return true;
    }
    off_by_one_digits(candidate, existing)
}

/// Same length, all digits, and every digit differs by the same ±1 step.
fn off_by_one_digits(a: &str, b: &str) -> bool {
    if a.len() != b.len() || a.is_empty() {
        return false;
    }
    let (da, db): (Vec<_>, Vec<_>) = (
        a.chars().map(|c| c.to_digit(10)).collect(),
        b.chars().map(|c| c.to_digit(10)).collect(),
    );
    if da.iter().chain(db.iter()).any(|d| d.is_none()) {
        return false;
    }
    let deltas: Vec<i64> = da
        .iter()
        .zip(db.iter())
        .map(|(x, y)| i64::from(x.unwrap()) - i64::from(y.unwrap()))
        .collect();
    matches!(deltas.first(), Some(&d) if (d == 1 || d == -1))
        && deltas.iter().all(|&d| d == deltas[0])
}

/// Validates a candidate duress PIN, client-side, before enrollment.
///
/// # What the distance rule covers, and what it does not
///
/// Invariant I3 asks for edit distance ≥2 between the duress PIN and the
/// regular PIN. Post-Tier-0 there are **no stored PIN hashes** — a PIN is
/// verified by decrypting the seed file — so "the regular PIN" is only
/// available in plaintext for the Cube the user currently has open, via
/// `app::session`.
///
/// So the rule is applied in the **narrowed form** decision D1(a) settles on,
/// and the narrowing is deliberate rather than an oversight:
///
/// - **≥2 against the Cube being enrolled from** — the plaintext is in hand, so
///   the full rule applies. This is the Cube whose PIN the user actually has in
///   muscle memory, and the one they would mistype.
/// - **exact-collision only against every other Cube** — enforced by
///   `duress_pin_collision_check_blocking`, which trial-decrypts each Cube's
///   seed. Distance cannot be computed there without the plaintext, and
///   enumerating the ~36 same-length neighbours per Cube would cost ~30 s each.
///
/// `regular_pin` is `None` when the session has no PIN. Enrollment then
/// **refuses**: silently skipping a security rule because a value was missing is
/// how the rule stops existing.
pub fn validate_duress_pin(
    duress_pin: &str,
    confirm: &str,
    regular_pin: Option<&str>,
) -> Result<(), String> {
    if duress_pin.is_empty() {
        return Err("Enter a duress PIN.".to_string());
    }
    if duress_pin.len() != 4 {
        return Err("Duress PIN must be exactly 4 digits.".to_string());
    }
    if !duress_pin.chars().all(|c| c.is_ascii_digit()) {
        return Err("Duress PIN must contain only digits.".to_string());
    }
    if duress_pin != confirm {
        return Err("The duress PINs don't match.".to_string());
    }

    let Some(regular) = regular_pin else {
        // Fail closed. The wizard runs inside an open Cube, so an absent
        // session PIN means something is wrong — not that the check is optional.
        return Err(
            "Coincube can't check your duress PIN against this Cube's unlock PIN right \
             now. Close and re-open the Cube, then try again."
                .to_string(),
        );
    };
    if too_similar(duress_pin, regular) {
        return Err(
            "That duress PIN is too close to this Cube's unlock PIN. Under pressure you \
             could type one meaning the other, and the duress PIN erases this device. \
             Choose something clearly different."
                .to_string(),
        );
    }
    Ok(())
}

/// Validates the all-clear passphrase: minimum length and distinctness from
/// the duress PIN.
pub fn validate_all_clear(passphrase: &str, duress_pin: &str) -> Result<(), String> {
    if passphrase.chars().count() < MIN_ALL_CLEAR_LEN {
        return Err(format!(
            "Your all-clear passphrase must be at least {} characters.",
            MIN_ALL_CLEAR_LEN
        ));
    }
    if too_similar(passphrase, duress_pin) {
        return Err(
            "Your all-clear passphrase must be clearly different from your duress PIN.".to_string(),
        );
    }
    Ok(())
}

/// Validates the account-level duress CRK decryption password (Approach C,
/// Tier 1 only): minimum length and distinctness from the duress PIN and
/// all-clear passphrase.
pub fn validate_duress_crk_password(
    password: &str,
    duress_pin: &str,
    all_clear: &str,
) -> Result<(), String> {
    if password.chars().count() < MIN_ALL_CLEAR_LEN {
        return Err(format!(
            "Your duress recovery password must be at least {} characters.",
            MIN_ALL_CLEAR_LEN
        ));
    }
    if too_similar(password, duress_pin) || too_similar(password, all_clear) {
        return Err(
            "Your duress recovery password must be clearly different from your other \
             credentials."
                .to_string(),
        );
    }
    Ok(())
}

/// A coarse entropy estimate (bits) for the entropy meter. Deliberately
/// simple — `len * log2(charset)` over the character classes present. Not a
/// substitute for a real strength estimator, but enough to drive a 0..1 meter.
pub fn estimate_entropy_bits(s: &str) -> f64 {
    let mut classes = 0u32;
    if s.chars().any(|c| c.is_ascii_lowercase()) {
        classes += 26;
    }
    if s.chars().any(|c| c.is_ascii_uppercase()) {
        classes += 26;
    }
    if s.chars().any(|c| c.is_ascii_digit()) {
        classes += 10;
    }
    if s.chars().any(|c| !c.is_ascii_alphanumeric()) {
        classes += 32;
    }
    if classes == 0 {
        return 0.0;
    }
    (s.chars().count() as f64) * (classes as f64).log2()
}

/// Generates this desktop's own ~128-bit duress code as a lowercase hex string,
/// using a cryptographically-secure RNG. The plaintext is held only on this
/// desktop; only its argon2id hash is sent to the server.
pub fn generate_duress_code() -> String {
    let mut bytes = [0u8; DURESS_CODE_BITS / 8];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// Argon2id-hashes a duress secret (code, all-clear, CRK password) into a PHC
/// string suitable for sending to the server. A fresh random salt is generated
/// per call.
pub fn hash_duress_secret(secret: &str) -> Result<String, String> {
    let salt = SaltString::generate(&mut ArgonOsRng);
    let params = Params::new(ARGON_M_COST, ARGON_T_COST, ARGON_P_COST, None)
        .map_err(|e| format!("argon2 params: {e}"))?;
    let argon2 = Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    let hash = argon2
        .hash_password(secret.as_bytes(), &salt)
        .map_err(|e| format!("argon2 hash: {e}"))?;
    Ok(hash.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The regular PIN of the Cube the wizard is running inside.
    const REGULAR: Option<&str> = Some("1234");

    #[test]
    fn duress_pin_requires_non_empty_and_matching_confirm() {
        assert!(validate_duress_pin("", "", REGULAR).is_err());
        // Mismatched confirmation is rejected.
        assert!(validate_duress_pin("1234", "1235", REGULAR).is_err());
        // A PIN clearly different from the regular one passes.
        assert!(validate_duress_pin("8765", "8765", REGULAR).is_ok());
    }

    /// **The canonical I3 regression.** Regular `1234`, duress `1235`.
    ///
    /// One keystroke apart. Under pressure the user types their own unlock PIN
    /// wrong and erases the device — the failure the distance rule exists for,
    /// and the one the exact-equality check let straight through.
    #[test]
    fn a_duress_pin_one_keystroke_from_the_real_one_is_rejected() {
        let err = validate_duress_pin("1235", "1235", REGULAR).unwrap_err();
        assert!(err.contains("too close"), "{}", err);

        // Exact collision, still rejected.
        assert!(validate_duress_pin("1234", "1234", REGULAR).is_err());

        // Distance 2 is the boundary and is accepted.
        assert_eq!(levenshtein("1234", "1256"), 2);
        assert!(validate_duress_pin("1256", "1256", REGULAR).is_ok());
    }

    #[test]
    fn reversed_and_off_by_one_are_rejected() {
        // Distance 4 by edit distance, but the same secret to a stressed user.
        assert_eq!(levenshtein("1234", "4321"), 4);
        assert!(validate_duress_pin("4321", "4321", REGULAR).is_err());

        // Every digit shifted by one. Distance 2 — it *passes* the ≥2 rule, so
        // only the explicit off-by-one check rejects it. Verified against a
        // reference implementation; an earlier version of this test asserted 4
        // and was simply wrong about the metric.
        assert_eq!(levenshtein("1234", "2345"), 2);
        assert!(validate_duress_pin("2345", "2345", REGULAR).is_err());
        assert!(validate_duress_pin("0123", "0123", REGULAR).is_err());

        // A genuinely unrelated PIN of the same shape is fine.
        assert!(validate_duress_pin("8092", "8092", REGULAR).is_ok());
    }

    /// Fail closed. A missing session PIN must refuse enrollment, not skip the
    /// rule — silently skipping is how a security rule stops existing.
    #[test]
    fn enrollment_refuses_when_the_regular_pin_is_unavailable() {
        let err = validate_duress_pin("8765", "8765", None).unwrap_err();
        assert!(err.contains("re-open the Cube"), "{}", err);
    }

    #[test]
    fn levenshtein_is_correct() {
        assert_eq!(levenshtein("", ""), 0);
        assert_eq!(levenshtein("1234", "1234"), 0);
        assert_eq!(levenshtein("1234", "1235"), 1);
        assert_eq!(levenshtein("1234", "123"), 1);
        assert_eq!(levenshtein("1234", "12345"), 1);
        assert_eq!(levenshtein("kitten", "sitting"), 3);
        assert_eq!(levenshtein("1234", "4321"), 4);
        assert_eq!(levenshtein("1234", "2345"), 2);
        assert_eq!(levenshtein("", "abc"), 3);
    }

    #[test]
    fn all_clear_length_and_distinctness() {
        assert!(validate_all_clear("short", "5678").is_err());
        assert!(validate_all_clear("correct horse battery", "5678").is_ok());
        assert!(validate_all_clear("1234", "1234").is_err());
        // Held to the same distance rule as the PIN, not just inequality.
        assert!(validate_all_clear("correct horse batter", "correct horse battery").is_err());
    }

    #[test]
    fn crk_password_distinct_from_everything() {
        assert!(
            validate_duress_crk_password("a-very-long-password", "5678", "all clear phrase")
                .is_ok()
        );
        assert!(validate_duress_crk_password(
            "all clear phrase here",
            "5678",
            "all clear phrase here"
        )
        .is_err());
        // And to the distance rule, not only equality.
        assert!(validate_duress_crk_password(
            "all clear phrase her",
            "5678",
            "all clear phrase here"
        )
        .is_err());
    }

    #[test]
    fn generated_code_is_128_bit_hex() {
        let code = generate_duress_code();
        assert_eq!(code.len(), 32, "128 bits == 32 hex chars");
        assert!(code.chars().all(|c| c.is_ascii_hexdigit()));
        // Two generations must differ (probability of collision is ~2^-128).
        assert_ne!(generate_duress_code(), generate_duress_code());
    }

    #[test]
    fn hash_round_trips_with_argon2() {
        use argon2::password_hash::{PasswordHash, PasswordVerifier};
        let code = generate_duress_code();
        let phc = hash_duress_secret(&code).unwrap();
        let parsed = PasswordHash::new(&phc).unwrap();
        assert!(Argon2::default()
            .verify_password(code.as_bytes(), &parsed)
            .is_ok());
        // Wrong secret fails.
        assert!(Argon2::default()
            .verify_password(b"not-the-code", &parsed)
            .is_err());
    }

    #[test]
    fn delays_are_in_minutes() {
        assert_eq!(DuressDelay::H24.minutes(), 1440);
        assert_eq!(DuressDelay::D90.minutes(), 129_600);
        assert_eq!(DuressDelay::default(), DuressDelay::H24);
        assert_eq!(DuressDelay::ALL.len(), 5);
    }

    #[test]
    fn entropy_grows_with_length_and_classes() {
        let weak = estimate_entropy_bits("aaaa");
        let strong = estimate_entropy_bits("Aa1!Aa1!Aa1!");
        assert!(strong > weak);
        assert_eq!(estimate_entropy_bits(""), 0.0);
    }
}

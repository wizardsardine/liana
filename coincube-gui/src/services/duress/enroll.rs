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
/// rest of the app.
const ARGON_M_COST: u32 = 19456;
const ARGON_T_COST: u32 = 2;
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

/// Validates a candidate duress PIN, client-side, before enrollment.
///
/// There is no longer a "distance from your regular PIN" rule: each Cube can
/// have its own PIN, so there's no single regular PIN to compare against. The
/// only hard requirement — that the duress PIN not collide with any Cube's
/// real unlock PIN — is enforced where the Cube PIN hashes are available, in
/// `persist_duress_enrollment`. Here we require a non-empty value entered
/// identically twice (the confirmation guards against memorizing a typo).
pub fn validate_duress_pin(duress_pin: &str, confirm: &str) -> Result<(), String> {
    if duress_pin.is_empty() {
        return Err("Enter a duress PIN.".to_string());
    }
    if duress_pin != confirm {
        return Err("The duress PINs don't match.".to_string());
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
    if passphrase == duress_pin {
        return Err(
            "Your all-clear passphrase must be different from your duress PIN.".to_string(),
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
    if password == duress_pin || password == all_clear {
        return Err(
            "Your duress recovery password must be different from your other credentials."
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

    #[test]
    fn duress_pin_requires_non_empty_and_matching_confirm() {
        assert!(validate_duress_pin("", "").is_err());
        // Mismatched confirmation is rejected.
        assert!(validate_duress_pin("1234", "1235").is_err());
        // Any non-empty PIN entered twice passes here — collision with a real
        // Cube PIN is enforced later, in `persist_duress_enrollment`.
        assert!(validate_duress_pin("1234", "1234").is_ok());
        assert!(validate_duress_pin("1235", "1235").is_ok());
    }

    #[test]
    fn all_clear_length_and_distinctness() {
        assert!(validate_all_clear("short", "5678").is_err());
        assert!(validate_all_clear("correct horse battery", "5678").is_ok());
        // Must differ from the duress PIN (edge: a 12+ char string equal to a
        // PIN can't happen with short PINs, but the rule still holds).
        assert!(validate_all_clear("1234", "1234").is_err());
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

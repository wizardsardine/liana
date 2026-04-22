//! Password-strength scoring for the Cube Recovery Kit password. The
//! plan (§2.3) gates the `Submit` action on a minimum strength of
//! `Fair`; this module produces a `PasswordStrength` enum plus a label
//! for the UI bar.
//!
//! Under the hood this wraps `zxcvbn` — zxcvbn's native score is 0..=4
//! so the enum mirrors that shape. We intentionally don't expose the
//! raw zxcvbn estimates (guesses, crack-time) in the API surface
//! because the UI only needs a coarse band.

use zeroize::Zeroizing;

/// Minimum password length enforced alongside strength. Set to the
/// NIST-recommended floor of 12 chars; zxcvbn will also penalise short
/// passwords in its score, but an explicit length gate keeps the
/// failure message crisp ("at least 12 characters").
pub const MIN_PASSWORD_LEN: usize = 12;

/// Coarse strength band shown to the user. Ordered so comparisons
/// (`strength >= PasswordStrength::Fair`) work as expected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PasswordStrength {
    /// Too short, too common, or trivially guessable (zxcvbn score 0).
    VeryWeak,
    /// Bruteforceable in under an hour online (score 1).
    Weak,
    /// Resistant to online throttled attacks; a reasonable default floor
    /// for a recovery password (score 2).
    Fair,
    /// Resistant to offline slow attacks (score 3).
    Strong,
    /// Resistant to offline fast-hardware attacks (score 4).
    VeryStrong,
}

impl PasswordStrength {
    pub fn label(self) -> &'static str {
        match self {
            Self::VeryWeak => "Very weak",
            Self::Weak => "Weak",
            Self::Fair => "Fair",
            Self::Strong => "Strong",
            Self::VeryStrong => "Very strong",
        }
    }

    /// Progress-bar fill on a 0.0–1.0 scale. Kept here rather than in
    /// the view module so the numeric mapping lives with the enum
    /// definition.
    pub fn fraction(self) -> f32 {
        match self {
            Self::VeryWeak => 0.1,
            Self::Weak => 0.3,
            Self::Fair => 0.5,
            Self::Strong => 0.75,
            Self::VeryStrong => 1.0,
        }
    }

    /// True when the strength meets the submit floor set by the plan
    /// (§2.3 — Submit gated on `strength >= medium`, which we interpret
    /// as `Fair`).
    pub fn is_acceptable(self) -> bool {
        self >= Self::Fair
    }
}

/// Scores `password` and returns the coarse band plus the first
/// feedback suggestion from zxcvbn, if any. An empty password is
/// treated as `VeryWeak` without invoking the full scorer.
///
/// The `user_inputs` slice is passed to zxcvbn so it can penalise
/// passwords that reuse pieces of identity data the caller already
/// knows (the user's email, the cube name). Callers should include
/// anything an attacker could trivially guess — that's exactly what
/// zxcvbn is designed to down-weight.
pub fn score(password: &Zeroizing<String>, user_inputs: &[&str]) -> (PasswordStrength, Option<String>) {
    if password.is_empty() {
        return (PasswordStrength::VeryWeak, None);
    }
    let est = zxcvbn::zxcvbn(password, user_inputs);
    let strength = match est.score() {
        zxcvbn::Score::Zero => PasswordStrength::VeryWeak,
        zxcvbn::Score::One => PasswordStrength::Weak,
        zxcvbn::Score::Two => PasswordStrength::Fair,
        zxcvbn::Score::Three => PasswordStrength::Strong,
        zxcvbn::Score::Four => PasswordStrength::VeryStrong,
        // zxcvbn::Score is #[non_exhaustive]; a future variant falls
        // back to the strongest bucket we know about — safer than
        // downgrading to VeryWeak on an upgrade.
        _ => PasswordStrength::VeryStrong,
    };
    let hint = est
        .feedback()
        .and_then(|f| {
            // Prefer a warning (specific failure mode) over the more
            // generic suggestions when both are present.
            if let Some(w) = f.warning() {
                Some(w.to_string())
            } else {
                f.suggestions().first().map(|s| s.to_string())
            }
        });
    (strength, hint)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn z(s: &str) -> Zeroizing<String> {
        Zeroizing::new(s.to_string())
    }

    #[test]
    fn empty_password_is_very_weak() {
        let (s, _) = score(&z(""), &[]);
        assert_eq!(s, PasswordStrength::VeryWeak);
    }

    #[test]
    fn common_password_is_weak_or_worse() {
        let (s, _) = score(&z("password"), &[]);
        assert!(s <= PasswordStrength::Weak, "got {:?}", s);
    }

    #[test]
    fn strong_random_passphrase_is_fair_or_better() {
        // Four uncommon words — the canonical "correct horse battery
        // staple" test. Should clear the Fair floor.
        let (s, _) = score(&z("correct horse battery staple"), &[]);
        assert!(s >= PasswordStrength::Fair, "got {:?}", s);
    }

    #[test]
    fn user_inputs_penalise_reused_values() {
        // Reusing the cube name as the password should be penalised.
        let (s1, _) = score(&z("MyCube!"), &[]);
        let (s2, _) = score(&z("MyCube!"), &["MyCube"]);
        assert!(
            s2 <= s1,
            "user_inputs hint should not increase score (s1={:?}, s2={:?})",
            s1,
            s2
        );
    }

    #[test]
    fn is_acceptable_gates_on_fair() {
        assert!(!PasswordStrength::VeryWeak.is_acceptable());
        assert!(!PasswordStrength::Weak.is_acceptable());
        assert!(PasswordStrength::Fair.is_acceptable());
        assert!(PasswordStrength::Strong.is_acceptable());
        assert!(PasswordStrength::VeryStrong.is_acceptable());
    }
}

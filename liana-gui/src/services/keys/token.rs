use liana::{bip39, getrandom, miniscript};
use miniscript::bitcoin::hashes::{sha256, Hash, HashEngine};
use std::error::Error;
use std::fmt::Display;
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

// OS-generated randomness. See https://docs.rs/getrandom/latest/getrandom/#supported-targets
// (basically this calls `getrandom()` or polls `/dev/urandom` on Linux, `BCryptGenRandom` on
// Windows, and `getentropy()` / `/dev/random` on Mac.
fn system_randomness() -> Result<[u8; 32], Box<dyn Error>> {
    let mut buf = [0; 32];
    getrandom::fill(&mut buf).map_err(|e| e.to_string())?;
    assert_ne!(buf, [0; 32]);
    Ok(buf)
}

fn get_random_1_to_999() -> Result<u16, Box<dyn Error>> {
    let mut buf = [0u8; 2];
    getrandom::fill(&mut buf).map_err(|e| e.to_string())?;
    let random_value = u16::from_le_bytes(buf);
    let random_number = (random_value as f64 / u16::MAX as f64 * 999.0).ceil() as u16;
    Ok(random_number)
}

pub fn random_bytes() -> Result<[u8; 32], Box<dyn Error>> {
    let mut engine = sha256::HashEngine::default();
    engine.input(&system_randomness()?);

    let timestamp: u16 =
        (SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() % u16::MAX as u64) as u16;
    engine.input(&timestamp.to_be_bytes());
    let pid = std::process::id();
    engine.input(&pid.to_be_bytes());

    Ok(sha256::Hash::from_engine(engine).to_byte_array())
}

#[derive(Debug, PartialEq)]
pub struct Token {
    number: u16,
    words: [String; 3],
}

impl Display for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}-{}-{}-{}",
            self.number, self.words[0], self.words[1], self.words[2]
        )
    }
}

impl FromStr for Token {
    type Err = &'static str;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut number = 0;
        let mut words = Vec::new();
        let values: Vec<&str> = s.split('-').collect();
        if values.len() != 4 {
            return Err("Wrong Token format");
        }
        for (i, value) in values.into_iter().enumerate() {
            if i == 0 {
                if let Ok(n) = u16::from_str(value) {
                    number = n;
                } else {
                    return Err("Wrong Token format");
                }
            } else if bip39::Language::English.find_word(value).is_some() {
                words.push(value);
            } else {
                return Err("Wrong Token format");
            }
        }

        Ok(Token {
            number,
            words: [
                words[0].to_string(),
                words[1].to_string(),
                words[2].to_string(),
            ],
        })
    }
}

pub fn generate_random_token() -> Result<Token, Box<dyn Error>> {
    let random_32bytes = random_bytes()?;
    let mnemonic = bip39::Mnemonic::from_entropy(&random_32bytes[..16])?;
    let mut words = mnemonic.words();
    let random_number = get_random_1_to_999()?;
    Ok(Token {
        number: random_number,
        words: [
            words
                .next()
                .expect("mnemonic has more than 3 words")
                .to_string(),
            words
                .next()
                .expect("mnemonic has more than 3 words")
                .to_string(),
            words
                .next()
                .expect("mnemonic has more than 3 words")
                .to_string(),
        ],
    })
}

#[cfg(test)]
mod tests {
    use liana::bip39;

    use super::*;
    use std::str::FromStr;

    // Test the Display trait implementation
    #[test]
    fn test_token_display() {
        let token = Token {
            number: 42,
            words: [
                "absent".to_string(),
                "cake".to_string(),
                "eagle".to_string(),
            ],
        };
        assert_eq!(token.to_string(), "42-absent-cake-eagle");
    }

    // Test successful FromStr parsing
    #[test]
    fn test_token_from_str_success() {
        let token_str = "42-absent-cake-eagle";
        let token = Token::from_str(token_str).expect("Should parse valid token");

        assert_eq!(token.number, 42);
        assert_eq!(token.words[0], "absent");
        assert_eq!(token.words[1], "cake");
        assert_eq!(token.words[2], "eagle");
    }

    // Test FromStr parsing failures
    #[test]
    fn test_token_from_str_failures() {
        // Wrong number of parts
        assert!(Token::from_str("42-absent-cake").is_err());
        assert!(Token::from_str("42-absent-cake-eagle-extra").is_err());

        // Invalid number
        assert!(Token::from_str("abc-absent-cake-eagle").is_err());

        // Invalid words
        assert!(Token::from_str("42-not-a-valid-word-test-words").is_err());
    }

    // Test generate_random_token function
    #[test]
    fn test_generate_random_token() {
        let token = generate_random_token().expect("Should generate a valid token");

        // Check number is within valid range
        assert!(token.number >= 1 && token.number <= 999);

        // Check words are valid BIP39 English words
        for word in &token.words {
            assert!(bip39::Language::English.find_word(word).is_some());
        }
    }

    // Test round-trip conversion (to_string and from_str)
    #[test]
    fn test_token_round_trip_conversion() {
        let original_token = Token {
            number: 123,
            words: [
                "absent".to_string(),
                "cake".to_string(),
                "eagle".to_string(),
            ],
        };

        let token_str = original_token.to_string();
        let parsed_token = Token::from_str(&token_str).expect("Should parse back to token");

        assert_eq!(original_token.number, parsed_token.number);
        assert_eq!(original_token.words, parsed_token.words);
    }
}

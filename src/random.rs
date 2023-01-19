use miniscript::bitcoin::hashes::{sha256, Hash, HashEngine};
use std::{
    collections::hash_map,
    error, fmt,
    hash::{BuildHasher, Hasher},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Debug)]
pub enum RandomnessError {
    Hardware(String),
    Os(String),
    ContextualInfo(String),
}

impl fmt::Display for RandomnessError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Hardware(s) => write!(f, "Error when getting randomness from hardware: {}", s),
            Self::Os(s) => write!(f, "Error when getting randomness from the OS: {}", s),
            Self::ContextualInfo(s) => write!(f, "Error when getting contextual info: {}", s),
        }
    }
}

impl error::Error for RandomnessError {}

// Get some entrop from RDRAND when available.
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
fn cpu_randomness() -> Result<Option<[u8; 32]>, RandomnessError> {
    if let Ok(mut rand_gen) = rdrand::RdRand::new() {
        let mut buf = [0; 32];
        rand_gen
            .try_fill_bytes(&mut buf)
            .map_err(|e| RandomnessError::Hardware(e.to_string()))?;
        assert_ne!(buf, [0; 32]);
        Ok(Some(buf))
    } else {
        // Not available.
        Ok(None)
    }
}

#[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
fn hardware_randomness() -> Result<Option<[u8; 32]>, RandomnessError> {
    Ok(None)
}

// OS-generated randomness. See https://docs.rs/getrandom/latest/getrandom/#supported-targets
// (basically this calls `getrandom()` or polls `/dev/urandom` on Linux, `BCryptGenRandom` on
// Windows, and `getentropy()` / `/dev/random` on Mac.
fn system_randomness() -> Result<[u8; 32], RandomnessError> {
    let mut buf = [0; 32];
    getrandom::getrandom(&mut buf).map_err(|e| RandomnessError::Os(e.to_string()))?;
    assert_ne!(buf, [0; 32]);
    Ok(buf)
}

// Some more contextual data to try to get at least a slight bit of additional entropy.
fn additional_data() -> Result<[u8; 32], RandomnessError> {
    let mut engine = sha256::HashEngine::default();

    let timestamp: u16 = (SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| RandomnessError::ContextualInfo(e.to_string()))?
        .as_secs()
        % u16::MAX as u64) as u16;
    engine.input(&timestamp.to_be_bytes());
    let hasher_number = hash_map::RandomState::new().build_hasher().finish();
    engine.input(&hasher_number.to_be_bytes());
    let pid = std::process::id();
    engine.input(&pid.to_be_bytes());
    // TODO: get some more contextual information

    Ok(*sha256::Hash::from_engine(engine).as_inner())
}

/// Get 32 random bytes. This is mainly based on OS-provided randomness (`getrandom` or
/// `/dev/urandom` on Linux, `getentropy` / `/dev/random` on MacOS, and `BCryptGenRandom` on
/// Windows. In addition some randomness may be taken directly from the CPU if it is
/// available, and some contextual information are added to the mix as well.
pub fn random_bytes() -> Result<[u8; 32], RandomnessError> {
    let mut engine = sha256::HashEngine::default();

    if let Some(bytes) = cpu_randomness()? {
        engine.input(&bytes);
    }
    engine.input(&system_randomness()?);
    engine.input(&additional_data()?);
    // TODO: add more sources of randomness

    Ok(*sha256::Hash::from_engine(engine).as_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    // This does not test the quality of the randomness but at least sanity checks it's
    // not obviously broken.
    #[test]
    fn randomness_sanity_check() {
        let mut set = HashSet::with_capacity(100);

        for _ in 0..100 {
            let rand = random_bytes().unwrap();
            assert!(!set.contains(&rand));
            set.insert(rand);
        }
    }
}

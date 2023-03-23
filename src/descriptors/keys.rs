use miniscript::{
    bitcoin::{
        self,
        hashes::{hash160, ripemd160, sha256},
        util::bip32,
    },
    hash256, MiniscriptKey, ToPublicKey,
};

use std::{error, fmt, str};

#[derive(Debug)]
pub enum DescKeyError {
    DerivedKeyParsing,
    InvalidMultiThresh(usize),
    InvalidMultiKeys(usize),
}

impl std::fmt::Display for DescKeyError {
    fn fmt(&self, f: &mut fmt::Formatter) -> std::fmt::Result {
        match self {
            DescKeyError::DerivedKeyParsing => write!(f, "Parsing derived key"),
            Self::InvalidMultiThresh(thresh) => write!(f, "Invalid threshold value '{}'. The threshold must be > to 0 and <= to the number of keys.", thresh),
            Self::InvalidMultiKeys(n_keys) => write!(f, "Invalid number of keys '{}'. Between 2 and 20 keys must be given to use multiple keys in a specific path.", n_keys),
        }
    }
}

impl error::Error for DescKeyError {}

/// A public key used in derived descriptors
#[derive(Debug, Eq, PartialEq, Clone, Ord, PartialOrd, Hash)]
pub struct DerivedPublicKey {
    /// Fingerprint of the master xpub and the derivation index used. We don't use a path
    /// since we never derive at more than one depth.
    pub origin: (bip32::Fingerprint, bip32::DerivationPath),
    /// The actual key
    pub key: bitcoin::PublicKey,
}

impl fmt::Display for DerivedPublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (fingerprint, deriv_path) = &self.origin;

        write!(f, "[")?;
        for byte in fingerprint.as_bytes().iter() {
            write!(f, "{:02x}", byte)?;
        }
        write!(f, "/{}", deriv_path)?;
        write!(f, "]{}", self.key)
    }
}

impl str::FromStr for DerivedPublicKey {
    type Err = DescKeyError;

    fn from_str(s: &str) -> Result<DerivedPublicKey, Self::Err> {
        // The key is always of the form:
        // [ fingerprint / index ]<key>

        // 1 + 8 + 1 + 1 + 1 + 66 minimum
        if s.len() < 78 {
            return Err(DescKeyError::DerivedKeyParsing);
        }

        // Non-ASCII?
        for ch in s.as_bytes() {
            if *ch < 20 || *ch > 127 {
                return Err(DescKeyError::DerivedKeyParsing);
            }
        }

        if s.chars().next().expect("Size checked above") != '[' {
            return Err(DescKeyError::DerivedKeyParsing);
        }

        let mut parts = s[1..].split(']');
        let fg_deriv = parts.next().ok_or(DescKeyError::DerivedKeyParsing)?;
        let key_str = parts.next().ok_or(DescKeyError::DerivedKeyParsing)?;

        if fg_deriv.len() < 10 {
            return Err(DescKeyError::DerivedKeyParsing);
        }
        let fingerprint = bip32::Fingerprint::from_str(&fg_deriv[..8])
            .map_err(|_| DescKeyError::DerivedKeyParsing)?;
        let deriv_path = bip32::DerivationPath::from_str(&fg_deriv[9..])
            .map_err(|_| DescKeyError::DerivedKeyParsing)?;
        if deriv_path.into_iter().any(bip32::ChildNumber::is_hardened) {
            return Err(DescKeyError::DerivedKeyParsing);
        }

        let key =
            bitcoin::PublicKey::from_str(key_str).map_err(|_| DescKeyError::DerivedKeyParsing)?;

        Ok(DerivedPublicKey {
            key,
            origin: (fingerprint, deriv_path),
        })
    }
}

impl MiniscriptKey for DerivedPublicKey {
    type Sha256 = sha256::Hash;
    type Hash256 = hash256::Hash;
    type Ripemd160 = ripemd160::Hash;
    type Hash160 = hash160::Hash;

    fn is_uncompressed(&self) -> bool {
        self.key.is_uncompressed()
    }

    fn is_x_only_key(&self) -> bool {
        false
    }

    fn num_der_paths(&self) -> usize {
        0
    }
}

impl ToPublicKey for DerivedPublicKey {
    fn to_public_key(&self) -> bitcoin::PublicKey {
        self.key
    }

    fn to_sha256(hash: &sha256::Hash) -> sha256::Hash {
        *hash
    }

    fn to_hash256(hash: &hash256::Hash) -> hash256::Hash {
        *hash
    }

    fn to_ripemd160(hash: &ripemd160::Hash) -> ripemd160::Hash {
        *hash
    }

    fn to_hash160(hash: &hash160::Hash) -> hash160::Hash {
        *hash
    }
}

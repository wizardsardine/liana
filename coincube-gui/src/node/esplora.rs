use std::fmt;

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum ConfigField {
    Address,
}

impl fmt::Display for ConfigField {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            ConfigField::Address => write!(f, "Esplora URL"),
        }
    }
}

/// Returns true if `value` looks like a plausible Esplora HTTP(S) URL.
pub fn is_esplora_address_valid(value: &str) -> bool {
    value.starts_with("http://") || value.starts_with("https://")
}

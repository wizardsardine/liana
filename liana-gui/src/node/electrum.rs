use std::fmt;

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum ConfigField {
    Address,
}

pub const ADDRESS_NOTES: &str = "Note: include \"ssl://\" as a prefix \
    for SSL connections. Be aware that self-signed \
    SSL certificates are currently not supported.";

impl fmt::Display for ConfigField {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            ConfigField::Address => write!(f, "RPC address"),
        }
    }
}

pub fn is_electrum_address_valid(value: &str) -> bool {
    let value_noprefix = if value.starts_with("ssl://") {
        value.replacen("ssl://", "", 1)
    } else {
        value.replacen("tcp://", "", 1)
    };
    let noprefix_parts: Vec<_> = value_noprefix.split(':').collect();
    noprefix_parts.len() == 2
        && !noprefix_parts
            .first()
            .expect("there are two parts")
            .is_empty()
        && noprefix_parts
            .last()
            .expect("there are two parts")
            .parse::<u16>() // check it is a port
            .is_ok()
}

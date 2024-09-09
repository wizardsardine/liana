use std::fmt;

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum ConfigField {
    Address,
}

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

use std::fmt;

use iced::{widget::checkbox, Element, Renderer};
use liana_ui::{component::form, theme::Theme};

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum ConfigField {
    Address,
}

pub const ADDRESS_NOTES: &str = "Note: include \"ssl://\" as a prefix \
    for SSL connections.";

pub const VALID_SSL_DOMAIN_NOTES: &str = "Do not validate SSL Domain \
    (check this only if you want to use a self-signed certificate)";

impl fmt::Display for ConfigField {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            ConfigField::Address => write!(f, "RPC address"),
        }
    }
}

pub fn validate_domain_checkbox<'a, F, M>(
    addr: &form::Value<String>,
    value: bool,
    closure: F,
) -> Option<Element<'a, M, Theme, Renderer>>
where
    F: 'a + Fn(bool) -> M,
    M: 'a,
{
    let checkbox = checkbox(VALID_SSL_DOMAIN_NOTES, !value).on_toggle(move |b| closure(!b));
    if addr.valid && is_ssl(&addr.value) {
        Some(checkbox.into())
    } else {
        None
    }
}

pub fn is_ssl(value: &str) -> bool {
    value.starts_with("ssl://")
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

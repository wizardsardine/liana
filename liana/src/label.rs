use std::{fmt, str::FromStr};

use miniscript::bitcoin::{Address, Network, OutPoint, Txid};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum LabelItem {
    Address(Address),
    Txid(Txid),
    OutPoint(OutPoint),
}

impl From<Address> for LabelItem {
    fn from(value: Address) -> Self {
        Self::Address(value)
    }
}

impl From<Txid> for LabelItem {
    fn from(value: Txid) -> Self {
        Self::Txid(value)
    }
}

impl From<OutPoint> for LabelItem {
    fn from(value: OutPoint) -> Self {
        Self::OutPoint(value)
    }
}

impl fmt::Display for LabelItem {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            LabelItem::Address(a) => write!(f, "{a}"),
            LabelItem::Txid(a) => write!(f, "{a}"),
            LabelItem::OutPoint(a) => write!(f, "{a}"),
        }
    }
}

impl LabelItem {
    pub fn from_str(s: &str, network: Network) -> Option<LabelItem> {
        if let Ok(addr) = Address::from_str(s) {
            if !addr.is_valid_for_network(network) {
                None
            } else {
                Some(LabelItem::Address(addr.assume_checked()))
            }
        } else if let Ok(txid) = Txid::from_str(s) {
            Some(LabelItem::Txid(txid))
        } else if let Ok(outpoint) = OutPoint::from_str(s) {
            Some(LabelItem::OutPoint(outpoint))
        } else {
            None
        }
    }

    pub fn from_bip329(label: &bip329::Label, network: Network) -> Option<(Self, String)> {
        match label {
            bip329::Label::Transaction(tx_record) => {
                if let (Some(txid), Some(label)) = (
                    Txid::from_str(&tx_record.ref_.to_string()).ok(),
                    tx_record.label.clone(),
                ) {
                    Some((Self::Txid(txid), label))
                } else {
                    None
                }
            }
            bip329::Label::Address(address_record) => {
                if let (Some(addr), Some(label)) = (
                    Address::from_str(&address_record.ref_.clone().assume_checked().to_string())
                        .ok(),
                    address_record.label.clone(),
                ) {
                    if addr.is_valid_for_network(network) {
                        Some((Self::Address(addr.assume_checked()), label))
                    } else {
                        None
                    }
                } else {
                    None
                }
            }
            bip329::Label::Output(output_record) => {
                if let (Some(outpoint), Some(label)) = (
                    OutPoint::from_str(&output_record.ref_.to_string()).ok(),
                    output_record.label.clone(),
                ) {
                    Some((Self::OutPoint(outpoint), label))
                } else {
                    None
                }
            }
            _ => None,
        }
    }
}

use liana_ui::{component::modal::legacy, widget::*};

use crate::{
    app::view::message::*,
    hw::{HardwareWallet, UnsupportedReason},
    t,
};
use async_hwi::DeviceKind;

pub fn hw_list_view_verify_address(
    i: usize,
    hw: &HardwareWallet,
    chosen: bool,
) -> Element<'_, Message> {
    match hw {
        HardwareWallet::Supported {
            kind,
            version,
            fingerprint,
            alias,
            ..
        } => {
            if chosen {
                legacy::processing_device(kind, version.as_ref(), fingerprint, alias.as_ref(), None)
            } else {
                match kind {
                    DeviceKind::Specter | DeviceKind::SpecterSimulator => {
                        legacy::unimplemented_method_device(
                            kind.to_string(),
                            version.as_ref(),
                            fingerprint,
                            t!("hw-display-address-unavailable"),
                            None,
                        )
                    }
                    _ => {
                        let select_msg = if hw.is_supported() {
                            Some(Message::SelectHardwareWallet(i))
                        } else {
                            None
                        };
                        legacy::supported_device(
                            kind,
                            version.as_ref(),
                            fingerprint,
                            alias.as_ref(),
                            select_msg,
                        )
                    }
                }
            }
        }
        HardwareWallet::Unsupported {
            version,
            kind,
            reason,
            ..
        } => match reason {
            UnsupportedReason::NotPartOfWallet(fg) => {
                legacy::unrelated_device(kind.to_string(), version.as_ref(), fg, None)
            }
            UnsupportedReason::WrongNetwork => {
                legacy::wrong_network_device(kind.to_string(), version.as_ref(), None)
            }
            UnsupportedReason::Version {
                minimal_supported_version,
            } => legacy::unsupported_version_device(
                kind.to_string(),
                version.as_ref(),
                minimal_supported_version,
                None,
            ),
            _ => legacy::unsupported_device(kind.to_string(), version.as_ref(), None),
        },
        HardwareWallet::Locked {
            kind, pairing_code, ..
        } => legacy::locked_device(kind, pairing_code.as_ref(), None),
    }
}

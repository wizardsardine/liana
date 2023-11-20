use iced::Length;

use liana_ui::{component::hw, theme, widget::*};

use crate::{app::view::message::*, hw::HardwareWallet};

pub fn hw_list_view(
    i: usize,
    hw: &HardwareWallet,
    chosen: bool,
    processing: bool,
    signed: bool,
) -> Element<Message> {
    let mut bttn = Button::new(match hw {
        HardwareWallet::Supported {
            kind,
            version,
            fingerprint,
            alias,
            registered,
            ..
        } => {
            if chosen && processing {
                hw::processing_hardware_wallet(kind, version.as_ref(), fingerprint, alias.as_ref())
            } else if signed {
                hw::sign_success_hardware_wallet(
                    kind,
                    version.as_ref(),
                    fingerprint,
                    alias.as_ref(),
                )
            } else if *registered == Some(false) {
                hw::unregistered_hardware_wallet(
                    kind,
                    version.as_ref(),
                    fingerprint,
                    alias.as_ref(),
                )
            } else {
                hw::supported_hardware_wallet(kind, version.as_ref(), fingerprint, alias.as_ref())
            }
        }
        HardwareWallet::Unsupported { version, kind, .. } => {
            hw::unsupported_hardware_wallet(&kind.to_string(), version.as_ref())
        }
        HardwareWallet::Locked {
            kind, pairing_code, ..
        } => hw::locked_hardware_wallet(kind, pairing_code.as_ref()),
    })
    .style(theme::Button::Border)
    .width(Length::Fill);
    if !processing {
        if let HardwareWallet::Supported { registered, .. } = hw {
            if *registered != Some(false) {
                bttn = bttn.on_press(Message::SelectHardwareWallet(i));
            }
        }
    }
    Container::new(bttn)
        .width(Length::Fill)
        .style(theme::Container::Card(theme::Card::Simple))
        .into()
}

pub fn hw_list_view_for_registration(
    i: usize,
    hw: &HardwareWallet,
    chosen: bool,
    processing: bool,
    registered: bool,
) -> Element<Message> {
    let mut bttn = Button::new(match hw {
        HardwareWallet::Supported {
            kind,
            version,
            fingerprint,
            alias,
            ..
        } => {
            if chosen && processing {
                hw::processing_hardware_wallet(kind, version.as_ref(), fingerprint, alias.as_ref())
            } else if registered {
                hw::registration_success_hardware_wallet(
                    kind,
                    version.as_ref(),
                    fingerprint,
                    alias.as_ref(),
                )
            } else {
                hw::supported_hardware_wallet(kind, version.as_ref(), fingerprint, alias.as_ref())
            }
        }
        HardwareWallet::Unsupported { version, kind, .. } => {
            hw::unsupported_hardware_wallet(&kind.to_string(), version.as_ref())
        }
        HardwareWallet::Locked {
            kind, pairing_code, ..
        } => hw::locked_hardware_wallet(kind, pairing_code.as_ref()),
    })
    .style(theme::Button::Border)
    .width(Length::Fill);
    if !processing && hw.is_supported() {
        bttn = bttn.on_press(Message::SelectHardwareWallet(i));
    }
    Container::new(bttn)
        .width(Length::Fill)
        .style(theme::Container::Card(theme::Card::Simple))
        .into()
}

pub fn hw_list_view_verify_address(
    i: usize,
    hw: &HardwareWallet,
    chosen: bool,
) -> Element<Message> {
    let mut bttn = Button::new(match hw {
        HardwareWallet::Supported {
            kind,
            version,
            fingerprint,
            alias,
            ..
        } => {
            if chosen {
                hw::processing_hardware_wallet(kind, version.as_ref(), fingerprint, alias.as_ref())
            } else {
                hw::supported_hardware_wallet(kind, version.as_ref(), fingerprint, alias.as_ref())
            }
        }
        HardwareWallet::Unsupported { version, kind, .. } => {
            hw::unsupported_hardware_wallet(&kind.to_string(), version.as_ref())
        }
        HardwareWallet::Locked {
            kind, pairing_code, ..
        } => hw::locked_hardware_wallet(kind, pairing_code.as_ref()),
    })
    .style(theme::Button::Border)
    .width(Length::Fill);
    if !chosen && hw.is_supported() {
        bttn = bttn.on_press(Message::SelectHardwareWallet(i));
    }
    Container::new(bttn)
        .width(Length::Fill)
        .style(theme::Container::Card(theme::Card::Simple))
        .into()
}

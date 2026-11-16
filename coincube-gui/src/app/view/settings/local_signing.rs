//! View for Vault → Settings → Pair with Keychain: the key the phone will
//! hold, the pairing steps, and the phones already paired.

use coincube_ui::locale::copy as localized_copy;
use coincube_ui::localized_format;
use iced::widget::{
    qr_code::{self, QRCode},
    Column, Container, Row, Space,
};
use iced::{Alignment, Length};

use coincube_ui::component::tenshu::{self, radio_list, segment, FieldSize, Tone};
use coincube_ui::component::text::*;
use coincube_ui::widget::Element;

use crate::app::cache;
use crate::app::menu::{Menu, SettingsOption, VaultSubMenu};
use crate::app::state::settings::local_signing::{LocalSigningState, PairableKey, PairingFlow};
use crate::app::view::dashboard;
use crate::app::view::message::{LocalSigningMessage, Message, SettingsMessage};
use crate::app::view::settings::page;
use crate::phone_signer::errors::PairingError;

fn local(message: LocalSigningMessage) -> Message {
    Message::Settings(SettingsMessage::LocalSigning(message))
}

const PAIR_ID: &str = "tenshu.settings.pair.start";
const PAIR_CANCEL_ID: &str = "tenshu.settings.pair.cancel";
const PAIR_RETRY_ID: &str = "tenshu.settings.pair.retry";
const RENAME_KEYS_ID: &str = "tenshu.settings.pair.rename-keys";
const DETAILS_ID: &str = "tenshu.settings.pair.details";

pub fn section<'a>(
    menu: &'a Menu,
    cache: &'a cache::Cache,
    state: &'a LocalSigningState,
) -> Element<'a, Message> {
    // Keychain keys only enter a Vault when it is created, so a Vault
    // without one can never pair a phone: say so once instead of laying out
    // empty key, pairing and phone groups.
    if no_keychain_key(state) && phones_for_vault(state).is_empty() {
        return dashboard(
            menu,
            cache,
            // No heading: it would only repeat the tab name.
            Column::new()
                .width(Length::Fill)
                .push(Space::new().height(Length::Fixed(page::GROUP_GAP)))
                .push(page::rows([page::row(localized_copy(
                    "This Vault has no Mobile key",
                ))
                .description(localized_copy(
                    "Phones sign with a key from COINCUBE Mobile, and that key \
                         can only be added when a Vault is created.",
                ))
                .into()])),
        );
    }
    let mut col = Column::new()
        .width(Length::Fill)
        .push(key_group(state))
        .push(pair_group(state))
        .push(paired_phones_group(state));

    if !matches!(state.flow, PairingFlow::Waiting { .. }) {
        if let Some(fp) = state.wallet_fingerprint {
            col = col.push(page::caption(localized_format!(
                "Phones paired here sign for this Vault (id {}). This identifier is derived \
                 from the Vault descriptor and is distinct from any signer's master fingerprint.", "Los teléfonos emparejados aquí firman para este Vault (id {}). Este identificador se deriva del descriptor Vault y es distinto de la huella maestra de cualquier firmante.",
                fp
            )));
        }
    }

    dashboard(menu, cache, col)
}

/// The Vault records its Keychain keys and has none.
fn no_keychain_key(state: &LocalSigningState) -> bool {
    state.keychain_keys_recorded && state.vault_keys.is_empty()
}

/// "Key for the phone": the Vault's Keychain keys as radio rows.
fn key_group<'a>(state: &'a LocalSigningState) -> Element<'a, Message> {
    if no_keychain_key(state) {
        return page::group(
            localized_copy("Key for the phone"),
            None,
            page::rows([page::row(localized_copy("No Mobile keys to pair"))
                .description(localized_copy(
                    "This Vault doesn't have a key from COINCUBE Mobile, so there's \
                     no phone that can sign for it. Mobile keys are added when you create a Vault.",
                ))
                .into()]),
        );
    }
    let selected = state
        .vault_keys
        .iter()
        .position(|key| state.selected_key.as_ref() == Some(&key.xpub));
    let xpubs: Vec<String> = state.vault_keys.iter().map(|k| k.xpub.clone()).collect();
    let keys = radio_list(
        state.vault_keys.iter().enumerate().map(|(index, key)| {
            let mut detail = key_subtitle(key).unwrap_or_default();
            if state.show_key_details {
                detail = format!("{}\n{}", detail, key.descriptor_key);
            }
            segment(
                iced::advanced::widget::Id::from(format!("tenshu.settings.pair.key.{}", index)),
                index,
                key_title(key),
            )
            .description(detail)
        }),
        selected,
        Message::FocusAction,
    )
    .on_select(move |index| local(LocalSigningMessage::SelectKey(xpubs[index].clone())));
    let content = Column::new()
        .width(Length::Fill)
        .push(keys)
        .push(page::rule())
        .push(Space::new().height(Length::Fixed(8.0)))
        .push(
            Row::new()
                .spacing(8)
                .align_y(Alignment::Center)
                .push(page::muted(localized_copy("Rename keys in")))
                .push(page::link(
                    RENAME_KEYS_ID,
                    localized_copy("Wallet & signers"),
                    Message::Menu(Menu::Vault(VaultSubMenu::Settings(Some(
                        SettingsOption::Wallet,
                    )))),
                ))
                .push(Space::new().width(Length::Fill))
                .push(page::link(
                    DETAILS_ID,
                    if state.show_key_details {
                        "Hide key details"
                    } else {
                        "Show key details"
                    },
                    local(LocalSigningMessage::ToggleKeyDetails),
                )),
        );
    page::group(localized_copy("Key for the phone"), None, content)
}

/// A key's display name: the name the user gave it, else a generic label
/// with its fingerprint, never the raw xpub.
fn key_title(key: &PairableKey) -> String {
    match (&key.name, key.fingerprint) {
        (Some(name), _) => name.clone(),
        (None, Some(fp)) if key.keychain => format!("Mobile key {}", fp),
        (None, Some(fp)) => format!("Key {}", fp),
        (None, None) => "Unnamed key".to_string(),
    }
}

/// The line under a key's name: where it comes from and the fingerprint
/// the phone shows for it.
fn key_subtitle(key: &PairableKey) -> Option<String> {
    match (key.keychain, key.fingerprint) {
        (true, Some(fp)) => Some(format!("Mobile · {}", fp)),
        (true, None) => Some("Mobile".to_string()),
        (false, Some(fp)) => Some(fp.to_string()),
        (false, None) => None,
    }
}

/// "Pair a phone": the current pairing step.
fn pair_group<'a>(state: &'a LocalSigningState) -> Element<'a, Message> {
    let body: Element<'a, Message> = match &state.flow {
        PairingFlow::Idle => idle_body(state),
        PairingFlow::PhonePicker { discovered } => picker_body(discovered),
        PairingFlow::Waiting { phone, offer, qr } => waiting_body(phone, offer, qr.as_ref()),
        PairingFlow::Error(e) => error_body(e),
    };
    page::group(
        localized_copy("Pair a phone"),
        Some(
            "Same Wi-Fi. No Connect account needed. Pairing does not move a key to the phone."
                .into(),
        ),
        body,
    )
}

fn idle_body<'a>(state: &'a LocalSigningState) -> Element<'a, Message> {
    if no_keychain_key(state) {
        return page::rows([page::row("")
            .line(page::status(
                page::Status::Off,
                "No Mobile-eligible key in this Vault",
            ))
            .control(page::button(PAIR_ID, "Pair phone", None))
            .into()]);
    }
    let ready = state.wallet_fingerprint.is_some() && state.selected_key.is_some();
    let hint = if state.selected_key.is_none() {
        "Select a key above, then open COINCUBE Mobile on the phone."
    } else {
        "Open COINCUBE Mobile on the phone first."
    };
    page::rows([page::row(hint)
        .control(page::primary(
            PAIR_ID,
            "Pair phone",
            ready.then(|| local(LocalSigningMessage::StartPairing)),
        ))
        .into()])
}

fn cancel<'a>(label: &'static str) -> Element<'a, Message> {
    page::button(
        PAIR_CANCEL_ID,
        label,
        Some(local(LocalSigningMessage::CancelPairing)),
    )
}

fn waiting_body<'a>(
    phone: &'a crate::phone_signer::mdns::DiscoveredPhone,
    offer: &'a crate::phone_signer::pairing::PairingOffer,
    qr: Option<&'a qr_code::Data>,
) -> Element<'a, Message> {
    let remaining = crate::phone_signer::pairing::seconds_remaining(offer);
    let mut info = Column::new()
        .spacing(4)
        .width(Length::Fill)
        .push(tenshu::label(localized_format!("Scan with COINCUBE Mobile on phone {}", "Escanea con COINCUBE Mobile en el teléfono {}", phone.cert_fp8)))
        .push(page::muted(if remaining > 0 {
            localized_format!(
                "Phone at {}. The phone and COINCUBE Desktop verify pairing automatically; no private key leaves this computer.", "Teléfono en {}. El teléfono y COINCUBE Desktop verifican emparejar automáticamente; ninguna llave privada deja este ordenador.",
                phone.addr
            )
        } else {
            "This offer can no longer be scanned. Start a fresh pairing offer.".to_string()
        }))
        .push(Space::new().height(Length::Fixed(6.0)))
        .push(if remaining > 0 {
            page::status(
                page::Status::Busy,
                localized_format!("Waiting · expires in {}s", "En espera · vence en {} s", remaining),
            )
        } else {
            page::status(page::Status::Bad, localized_copy("Pairing offer expired"))
        })
        .push(Space::new().height(Length::Fixed(6.0)))
        .push(cancel("Cancel pairing"));
    let mut row = Row::new().spacing(24).align_y(Alignment::Start);
    if let Some(qr) = qr.filter(|_| remaining > 0) {
        row = row.push(
            Container::new(QRCode::<coincube_ui::theme::Theme>::new(qr).cell_size(4))
                .padding(8)
                .style(|_| iced::widget::container::Style {
                    background: Some(iced::Background::Color(iced::Color::WHITE)),
                    border: iced::Border {
                        radius: 8.0.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                }),
        );
    }
    info = info.width(Length::Fill);
    page::rows([
        Container::new(row.push(info))
            .padding([16, 0])
            .width(Length::Fill)
            .into(),
        page::rule(),
    ])
}

fn picker_body<'a>(
    discovered: &'a [crate::phone_signer::mdns::DiscoveredPhone],
) -> Element<'a, Message> {
    let mut rows: Vec<Element<'a, Message>> = Vec::new();
    for (i, d) in discovered.iter().enumerate() {
        let fp8 = d.cert_fp8.clone();
        let press = Some(local(LocalSigningMessage::PickPhone(fp8)));
        let id =
            iced::advanced::widget::Id::from(format!("tenshu.settings.pair.phone.{}", d.cert_fp8));
        rows.push(
            page::row(localized_format!("Phone {}", "Teléfono {}", d.cert_fp8))
                .description(d.addr.to_string())
                .control(if i == 0 {
                    page::primary(id, localized_copy("Pair"), press)
                } else {
                    page::button(id, localized_copy("Pair"), press)
                })
                .into(),
        );
    }
    Column::new()
        .width(Length::Fill)
        .push(page::status(page::Status::Busy, localized_copy("Looking for phones on this Wi-Fi…")))
        .push(Space::new().height(Length::Fixed(12.0)))
        .push(page::rows(rows))
        .push(Space::new().height(Length::Fixed(12.0)))
        .push(
            Row::new()
                .spacing(16)
                .align_y(Alignment::Center)
                .push(
                    Container::new(page::muted(if discovered.is_empty() {
                        localized_copy("No phones yet. Open COINCUBE Mobile on a phone on the same network and keep it unlocked.")
                    } else {
                        localized_copy("Not listed? Check both are on the same Wi-Fi.")
                    }))
                    .width(Length::Fill),
                )
                .push(cancel("Cancel")),
        )
        .into()
}

fn error_body<'a>(err: &'a PairingError) -> Element<'a, Message> {
    let (title, body) = match err {
        PairingError::OfferExpired => (
            localized_copy("Offer expired"),
            localized_copy(
                "The pairing offer ran out before any phone scanned it. \
                 Generate a new offer and try again.",
            )
            .to_string(),
        ),
        PairingError::WalletFingerprintMismatch { expected, claimed } => (
            localized_copy("Wrong wallet"),
            localized_format!(
                "The phone reports it can sign for wallet {} but \
                 this wallet expects {}. Confirm the phone is paired \
                 with the right vault.",
                "El teléfono indica que puede firmar para la cartera {}, \
                 pero esta cartera espera {}. Confirma que el teléfono \
                 esté vinculado con el Vault correcto.",
                claimed,
                expected
                    .iter()
                    .map(|fp| fp.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ),
        PairingError::ReplayRefused => (
            localized_copy("QR already used"),
            localized_copy(
                "This pairing QR has already completed a pairing. Go \
                 back and start a fresh offer rather than re-using it.",
            )
            .to_string(),
        ),
        PairingError::PhoneVerificationFailed => (
            localized_copy("Couldn't verify the phone"),
            localized_copy(
                "The device that scanned the QR couldn't prove it's the \
                 phone you're pairing. This can happen on an untrusted \
                 network. Make sure both devices are on a network you \
                 trust, then start a fresh pairing.",
            )
            .to_string(),
        ),
        PairingError::TransportKeyMissing => (
            localized_copy("Mobile needs updating"),
            localized_copy(
                "This phone didn't send an encryption key, so signing requests \
                 to it couldn't be encrypted. Update COINCUBE Mobile on the phone, \
                 then pair again.",
            )
            .to_string(),
        ),
        PairingError::NetworkError(s) => (
            localized_copy("Network error"),
            localized_format!(
                "Could not finish the TLS handshake or stream the \
                 pairing envelope. Confirm both devices are on the \
                 same Wi-Fi and try again. ({})",
                "No se pudo completar el protocolo TLS ni transmitir el \
                 sobre de vinculación. Confirma que ambos dispositivos \
                 estén en la misma red Wi-Fi e inténtalo de nuevo. ({})",
                s
            ),
        ),
        PairingError::InternalError(s) => (
            localized_copy("Pairing failed"),
            coincube_ui::locale::notice(s.clone()),
        ),
    };
    let mut actions = Row::new().spacing(8);
    if err.is_retriable() {
        actions = actions.push(page::primary(
            PAIR_RETRY_ID,
            localized_copy("Try again"),
            Some(local(LocalSigningMessage::StartPairing)),
        ));
    }
    actions = actions.push(cancel("Cancel"));
    page::note(
        Tone::Bad,
        Column::new()
            .spacing(2)
            .push(text(title).size(13).bold())
            .push(page::muted(body)),
        Some(actions.into()),
    )
}

/// "Paired phones": each phone paired with this Vault, its binding status,
/// name, fallback address and removal.
/// Pairing is vault-scoped: only phones paired with the currently-loaded
/// vault, so the list agrees with the pairing group's "sign for this Vault"
/// copy and the hw refresh loop's scoping.
fn phones_for_vault(
    state: &LocalSigningState,
) -> Vec<&crate::phone_signer::pairing_store::PairedPhone> {
    match state.wallet_fingerprint {
        Some(vid) => state
            .phones
            .phones
            .iter()
            .filter(|p| p.vault_fingerprint == vid)
            .collect(),
        None => Vec::new(),
    }
}

fn paired_phones_group<'a>(state: &'a LocalSigningState) -> Element<'a, Message> {
    let phones_for_vault = phones_for_vault(state);

    let mut rows: Vec<Element<'a, Message>> = Vec::new();
    if phones_for_vault.is_empty() {
        rows.push(
            page::row(localized_copy("No phones paired yet."))
                .description(localized_copy(
                    "Paired phones appear as a signer whenever they're reachable on your Wi-Fi.",
                ))
                .into(),
        );
    }
    for p in phones_for_vault {
        let fp8 = crate::phone_signer::identity::pin_hex8(&p.cert_pin);
        let draft = state.row_drafts.get(&fp8);
        let name_value = draft.map(|d| d.name.as_str()).unwrap_or(&p.name);
        let fallback_value = draft
            .map(|d| d.fallback.as_str())
            .unwrap_or_else(|| p.fallback_addr.as_deref().unwrap_or(""));
        let fp8_for_name = fp8.clone();
        let fp8_for_fb = fp8.clone();
        let fp8_for_save = fp8.clone();
        // Must be the same predicate signing and the hw refresh loop apply.
        let has_full_binding = p
            .signer_binding
            .as_ref()
            .is_some_and(|binding| binding.descriptor_sha256.len() == 32);
        let removable = p.signer_binding.as_ref().is_some_and(|binding| {
            binding.descriptor_sha256.len() == 32
                && hex::encode(&binding.descriptor_sha256) == state.descriptor_sha256
        });
        let exact = p
            .exact_signer_against(&state.descriptor_sha256, &state.vault_key_fingerprints)
            .is_ok();
        let (kind, word, detail) = if exact {
            (page::Status::Ok, "Paired", "Exact vault key paired.")
        } else if has_full_binding && !removable {
            (
                page::Status::Warn,
                "Other Vault",
                "This pairing belongs to a different Vault descriptor. Open that Vault to remove it.",
            )
        } else {
            (
                page::Status::Warn,
                "Pair again",
                "Pair again and select the exact vault key held by this phone. If the phone is lost, a separate recovery flow is needed.",
            )
        };
        rows.push(
            page::row(if p.name.is_empty() {
                localized_copy("Phone")
            } else {
                p.name.as_str()
            })
            .accessory(page::status(kind, word))
            .line(tenshu::mono(fp8.clone()))
            .description(detail)
            .control(page::button(
                iced::advanced::widget::Id::from(format!("tenshu.settings.pair.{}.unpair", fp8)),
                "Unpair…",
                removable.then(|| local(LocalSigningMessage::ReviewRemovePhone(fp8.clone()))),
            ))
            .into(),
        );
        let field = |id: String, label: &'static str, value: &'a str| {
            tenshu::text_field(
                iced::advanced::widget::Id::from(id),
                label,
                value,
                Message::FocusAction,
            )
            .size(FieldSize::Compact)
            .width(Length::Fill)
        };
        let edit = Row::new()
            .spacing(12)
            .align_y(Alignment::End)
            .push(
                field(
                    format!("tenshu.settings.pair.{}.name", fp8),
                    coincube_ui::locale::copy("Phone name"),
                    name_value,
                )
                .on_input(move |s| local(LocalSigningMessage::DraftName(fp8_for_name.clone(), s))),
            )
            .push(
                field(
                    format!("tenshu.settings.pair.{}.fallback", fp8),
                    coincube_ui::locale::copy("Fallback host:port"),
                    fallback_value,
                )
                .placeholder(localized_copy("For networks that block mDNS"))
                .on_input(move |s| {
                    local(LocalSigningMessage::DraftFallback(fp8_for_fb.clone(), s))
                }),
            )
            .push(page::button(
                iced::advanced::widget::Id::from(format!("tenshu.settings.pair.{}.save", fp8)),
                localized_copy("Save"),
                Some(local(LocalSigningMessage::SaveRow(fp8_for_save))),
            ));
        let mut detail_col = Column::new().spacing(10).push(edit);
        if state.pending_remove_phone.as_deref() == Some(fp8.as_str()) {
            detail_col = detail_col.push(page::note(
                Tone::Bad,
                Column::new()
                    .spacing(2)
                    .push(text(localized_copy("Remove this local pairing?")).size(13).bold())
                    .push(page::muted(
                        localized_copy("This removes the phone from this desktop's paired list and tries to notify it if reachable. It does not delete the key held on the phone or prove remote removal."),
                    )),
                Some(
                    Row::new()
                        .spacing(8)
                        .push(page::primary(
                            "tenshu.settings.pair.remove",
                            localized_copy("Remove local pairing"),
                            Some(local(LocalSigningMessage::RemovePhone(fp8.clone()))),
                        ))
                        .push(page::button(
                            "tenshu.settings.pair.keep",
                            localized_copy("Keep phone"),
                            Some(local(LocalSigningMessage::CancelRemovePhone)),
                        ))
                        .into(),
                ),
            ));
        }
        rows.push(
            Column::new()
                .push(page::disclosure_body([Container::new(detail_col)
                    .padding([12, 0])
                    .width(Length::Fill)
                    .into()]))
                .push(page::rule())
                .into(),
        );
    }

    let mut content = Column::new().width(Length::Fill);
    if let Some(notice) = state.unpaired_notice.as_ref() {
        content = content
            .push(page::note(
                Tone::Muted,
                page::muted(coincube_ui::locale::notice(notice.clone())),
                None,
            ))
            .push(Space::new().height(Length::Fixed(12.0)));
    }
    page::group(
        localized_copy("Paired phones"),
        None,
        content.push(page::rows(rows)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use coincube_core::miniscript::bitcoin::bip32::Fingerprint;
    use std::str::FromStr;

    fn key(name: Option<&str>, keychain: bool) -> PairableKey {
        PairableKey {
            xpub: "tpubD6NzVbkrYhZ4X".to_string(),
            descriptor_key: "[f108451f/48'/1'/0'/2']tpubD6NzVbkrYhZ4X".to_string(),
            fingerprint: Some(Fingerprint::from_str("f108451f").unwrap()),
            name: name.map(str::to_string),
            keychain,
        }
    }

    #[test]
    fn keys_are_labelled_by_name_and_never_by_xpub() {
        let named = key(Some("My iPhone"), true);
        assert_eq!(key_title(&named), "My iPhone");
        assert_eq!(key_subtitle(&named).as_deref(), Some("Mobile · f108451f"));

        let unnamed = key(None, true);
        assert_eq!(key_title(&unnamed), "Mobile key f108451f");

        let other = key(None, false);
        assert_eq!(key_title(&other), "Key f108451f");
        assert_eq!(key_subtitle(&other).as_deref(), Some("f108451f"));

        for k in [named, unnamed, other] {
            assert!(!key_title(&k).contains("tpub"));
            assert!(!key_subtitle(&k).unwrap_or_default().contains("tpub"));
        }
    }

    #[test]
    fn idle_body_renders_both_the_empty_state_and_a_key_list() {
        let mut state = LocalSigningState::default();
        state.keychain_keys_recorded = true;
        let _ = idle_body(&state);

        state.vault_keys = vec![key(Some("My iPhone"), true)];
        state.show_key_details = true;
        let _ = idle_body(&state);

        state.selected_key = Some(state.vault_keys[0].xpub.clone());
        let _ = idle_body(&state);
    }

    #[test]
    fn a_vault_without_a_keychain_key_shows_one_note_instead_of_pairing() {
        use crate::app::view::shared::landing::capture;

        let menu = Menu::Vault(VaultSubMenu::Settings(Some(SettingsOption::LocalSigning)));
        let cache = cache::Cache::default();
        let render = |state: &LocalSigningState| {
            capture::inspect(
                "settings-pair-keychain",
                section(&menu, &cache, state),
                (1440, 900),
                "dark",
                coincube_ui::theme::Theme::dark(),
            )
        };

        let mut state = LocalSigningState::default();
        state.keychain_keys_recorded = true;
        let empty = render(&state);
        assert!(empty.contains("This Vault has no Mobile key"));
        for gone in ["Key for the phone", "Pair a phone", "Paired phones"] {
            assert!(!empty.contains(gone), "{}", gone);
        }

        state.vault_keys = vec![key(Some("My iPhone"), true)];
        let pairable = render(&state);
        assert!(!pairable.contains("This Vault has no Mobile key"));
        for shown in ["Key for the phone", "Pair a phone", "Paired phones"] {
            assert!(pairable.contains(shown), "{}", shown);
        }
    }

    #[test]
    fn every_pairing_step_renders() {
        let phone = crate::phone_signer::mdns::DiscoveredPhone {
            cert_fp8: "65647abe".into(),
            addr: "192.168.100.95:54502".parse().unwrap(),
            instance_name: "x".into(),
        };
        let _ = picker_body(&[]);
        let _ = picker_body(std::slice::from_ref(&phone));
        let _ = error_body(&PairingError::ReplayRefused);
        let _ = error_body(&PairingError::OfferExpired);
    }
}

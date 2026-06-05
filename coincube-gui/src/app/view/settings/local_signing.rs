//! View for the "Paired phones" / local LAN signer settings section.

use iced::widget::{
    qr_code::{self, QRCode},
    text_input, Column, Container, Row, Space,
};
use iced::{Alignment, Font, Length};

use coincube_ui::component::{badge, button, card, separation, text::*};
use coincube_ui::widget::Element;
use coincube_ui::{icon, theme};

use crate::app::cache;
use crate::app::menu::Menu;
use crate::app::state::settings::local_signing::{LocalSigningState, PairingFlow};
use crate::app::view::dashboard;
use crate::app::view::message::{LocalSigningMessage, Message, SettingsMessage};
use crate::phone_signer::errors::PairingError;

pub fn section<'a>(
    menu: &'a Menu,
    cache: &'a cache::Cache,
    state: &'a LocalSigningState,
) -> Element<'a, Message> {
    let mut col = Column::new()
        .spacing(20)
        .push(super::header(
            "Pair with Keychain",
            SettingsMessage::LocalSigningSection,
        ))
        .push(pairing_card(state))
        .push(paired_phones_card(state))
        .width(Length::Fill);

    if !matches!(state.flow, PairingFlow::Waiting { .. }) {
        if let Some(fp) = state.wallet_fingerprint {
            col = col.push(
                text(format!(
                    "Phones paired through this panel will sign for this vault \
                     (id {}). This identifier is derived from the vault \
                     descriptor and is distinct from any individual signer's \
                     master fingerprint.",
                    fp
                ))
                .style(theme::text::secondary),
            );
        }
    }

    dashboard(menu, cache, col)
}

fn pairing_card<'a>(state: &'a LocalSigningState) -> Element<'a, Message> {
    let header = Row::new()
        .push(badge::badge(icon::tooltip_icon()))
        .push(text("Pair a phone").bold())
        .padding(10)
        .spacing(20)
        .align_y(Alignment::Center)
        .width(Length::Fill);

    let body: Element<'a, Message> = match &state.flow {
        PairingFlow::Idle => idle_body(state),
        PairingFlow::PhonePicker { discovered } => picker_body(discovered),
        PairingFlow::Waiting { phone, offer, qr } => waiting_body(phone, offer, qr.as_ref()),
        PairingFlow::Error(e) => error_body(e),
    };

    card::simple(
        Column::new()
            .push(header)
            .push(separation().width(Length::Fill))
            .push(Space::new().height(Length::Fixed(10.0)))
            .push(body),
    )
    .into()
}

fn idle_body<'a>(state: &'a LocalSigningState) -> Element<'a, Message> {
    let mut pair_btn = button::secondary(None, "Pair phone");
    if state.wallet_fingerprint.is_some() {
        pair_btn = pair_btn.on_press(Message::Settings(SettingsMessage::LocalSigning(
            LocalSigningMessage::StartPairing,
        )));
    }
    Column::new()
        .padding(10)
        .spacing(8)
        .push(text(
            "Pair a Keychain phone over your local network so it can \
             sign PSBTs directly, without going through the Connect \
             API. The phone must be on the same Wi-Fi.",
        ))
        .push(pair_btn)
        .into()
}

fn waiting_body<'a>(
    phone: &'a crate::phone_signer::mdns::DiscoveredPhone,
    offer: &'a crate::phone_signer::pairing::PairingOffer,
    qr: Option<&'a qr_code::Data>,
) -> Element<'a, Message> {
    let remaining = crate::phone_signer::pairing::seconds_remaining(offer);
    let countdown = if remaining > 0 {
        format!("Waiting for {} — expires in {}s", phone.cert_fp8, remaining)
    } else {
        "Pairing offer expired.".to_string()
    };

    let mut body = Column::new()
        .padding(10)
        .spacing(10)
        .align_x(Alignment::Center)
        .push(text(countdown))
        .push(text(format!(
            "Scan this QR with the Keychain app on \
             phone {} ({}).",
            phone.cert_fp8, phone.addr,
        )));
    if let Some(qr) = qr {
        body = body.push(
            Container::new(QRCode::<coincube_ui::theme::Theme>::new(qr).cell_size(8)).padding(10),
        );
    }
    body = body.push(
        button::secondary(None, "Cancel").on_press(Message::Settings(
            SettingsMessage::LocalSigning(LocalSigningMessage::CancelPairing),
        )),
    );
    body.into()
}

fn picker_body<'a>(
    discovered: &'a [crate::phone_signer::mdns::DiscoveredPhone],
) -> Element<'a, Message> {
    if discovered.is_empty() {
        return Column::new()
            .padding(10)
            .spacing(8)
            .push(text("Looking for phones…").bold())
            .push(text(
                "No phones found on this Wi-Fi yet. Open the Keychain \
                 app on a phone on the same network and make sure it's \
                 unlocked, then wait a few seconds.",
            ))
            .push(
                button::secondary(None, "Cancel").on_press(Message::Settings(
                    SettingsMessage::LocalSigning(LocalSigningMessage::CancelPairing),
                )),
            )
            .into();
    }
    let mut col = Column::new()
        .padding(10)
        .spacing(8)
        .push(text("Pick a phone to pair with").bold())
        .push(text(
            "These are the Keychain phones currently advertising on \
             this Wi-Fi. The list refreshes every second.",
        ));
    for d in discovered {
        let fp8 = d.cert_fp8.clone();
        let row = Row::new()
            .padding([6, 0])
            .spacing(12)
            .align_y(Alignment::Center)
            .push(
                Column::new()
                    .push(text(format!("Phone {}", d.cert_fp8)).bold())
                    .push(text(format!("{}", d.addr)).style(theme::text::secondary))
                    .width(Length::FillPortion(3)),
            )
            .push(button::secondary(None, "Pair").on_press(Message::Settings(
                SettingsMessage::LocalSigning(LocalSigningMessage::PickPhone(fp8)),
            )));
        col = col.push(row);
    }
    col = col.push(separation().width(Length::Fill));
    col = col.push(
        button::secondary(None, "Cancel").on_press(Message::Settings(
            SettingsMessage::LocalSigning(LocalSigningMessage::CancelPairing),
        )),
    );
    col.into()
}

fn error_body<'a>(err: &'a PairingError) -> Element<'a, Message> {
    let (title, body) = match err {
        PairingError::OfferExpired => (
            "Offer expired",
            "The pairing offer ran out before any phone scanned it. \
             Generate a new offer and try again."
                .to_string(),
        ),
        PairingError::WalletFingerprintMismatch { expected, claimed } => (
            "Wrong wallet",
            format!(
                "The phone reports it can sign for wallet {} but \
                 this wallet expects {}. Confirm the phone is paired \
                 with the right vault.",
                claimed,
                expected
                    .iter()
                    .map(|fp| fp.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ),
        PairingError::ReplayRefused => (
            "QR already used",
            "This pairing QR has already completed a pairing. Cancel \
             and start a fresh offer rather than re-using it."
                .to_string(),
        ),
        PairingError::PhoneVerificationFailed => (
            "Couldn't verify the phone",
            "The device that scanned the QR couldn't prove it's the \
             phone you're pairing. This can happen on an untrusted \
             network. Make sure both devices are on a network you \
             trust, then start a fresh pairing."
                .to_string(),
        ),
        PairingError::NetworkError(s) => (
            "Network error",
            format!(
                "Could not finish the TLS handshake or stream the \
                 pairing envelope. Confirm both devices are on the \
                 same Wi-Fi and try again. ({})",
                s
            ),
        ),
        PairingError::InternalError(s) => ("Pairing failed", s.clone()),
    };

    let mut col = Column::new()
        .padding(10)
        .spacing(8)
        .push(text(title).bold().style(theme::text::error))
        .push(text(body).style(theme::text::error));
    if err.is_retriable() {
        col = col.push(
            button::secondary(None, "Try again").on_press(Message::Settings(
                SettingsMessage::LocalSigning(LocalSigningMessage::StartPairing),
            )),
        );
    }
    col.into()
}

fn paired_phones_card<'a>(state: &'a LocalSigningState) -> Element<'a, Message> {
    let header = Row::new()
        .push(badge::badge(icon::tooltip_icon()))
        .push(text("Paired phones").bold())
        .padding(10)
        .spacing(20)
        .align_y(Alignment::Center)
        .width(Length::Fill);

    let body: Element<'a, Message> = if state.phones.phones.is_empty() {
        text(
            "No paired phones yet. Use 'Pair phone' above to add one. \
             Paired phones appear as a signer whenever they're \
             reachable on your Wi-Fi.",
        )
        .style(theme::text::secondary)
        .into()
    } else {
        let mut rows = Column::new().padding(10).spacing(10);
        for p in &state.phones.phones {
            let fp8 = crate::phone_signer::identity::pin_hex8(&p.cert_pin);
            let draft = state.row_drafts.get(&fp8);
            let name_value = draft.map(|d| d.name.as_str()).unwrap_or(&p.name);
            let fallback_value = draft
                .map(|d| d.fallback.as_str())
                .unwrap_or_else(|| p.fallback_addr.as_deref().unwrap_or(""));
            let fp8_for_name = fp8.clone();
            let fp8_for_fb = fp8.clone();
            let fp8_for_save = fp8.clone();
            let row = Column::new()
                .padding([6, 0])
                .spacing(6)
                .push(
                    Row::new()
                        .spacing(8)
                        .align_y(Alignment::Center)
                        .push(
                            text_input("Phone name", name_value)
                                .on_input(move |s| {
                                    Message::Settings(SettingsMessage::LocalSigning(
                                        LocalSigningMessage::DraftName(fp8_for_name.clone(), s),
                                    ))
                                })
                                .width(Length::FillPortion(3)),
                        )
                        .push(
                            Container::new(
                                text(fp8.clone())
                                    .size(P2_SIZE)
                                    .font(Font::MONOSPACE)
                                    .style(theme::text::secondary),
                            )
                            .padding([2, 6])
                            .width(Length::Shrink),
                        ),
                )
                .push(
                    Row::new()
                        .spacing(8)
                        .align_y(Alignment::Center)
                        .push(
                            text_input(
                                "Fallback host:port (mDNS-blocked networks)",
                                fallback_value,
                            )
                            .on_input(move |s| {
                                Message::Settings(SettingsMessage::LocalSigning(
                                    LocalSigningMessage::DraftFallback(fp8_for_fb.clone(), s),
                                ))
                            })
                            .width(Length::FillPortion(3)),
                        )
                        .push(
                            button::secondary(None, "Save")
                                .on_press(Message::Settings(SettingsMessage::LocalSigning(
                                    LocalSigningMessage::SaveRow(fp8_for_save),
                                )))
                                .width(Length::Shrink),
                        )
                        .push(
                            button::secondary(None, "Remove")
                                .on_press(Message::Settings(SettingsMessage::LocalSigning(
                                    LocalSigningMessage::RemovePhone(fp8.clone()),
                                )))
                                .width(Length::Shrink),
                        ),
                );
            rows = rows.push(row);
            rows = rows.push(separation().width(Length::Fill));
        }
        rows.into()
    };

    card::simple(
        Column::new()
            .push(header)
            .push(separation().width(Length::Fill))
            .push(Space::new().height(Length::Fixed(10.0)))
            .push(body),
    )
    .into()
}

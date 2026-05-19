//! Renders the installer's "Decrypt backup file" modal
//! (`installer::decrypt::DecryptModal`) across the visual states
//! reachable via its public `update()` API.
//!
//! Bytes are loaded from the test fixture
//! `liana-gui/test_assets/v0.bed`, so most pages land on the
//! `valid_content` branch of `decrypt_view`. The error page passes
//! empty bytes to surface the `InvalidEncoding` arm. Branches that
//! depend on private fields (in-flight HW fetches, the nested
//! `ExportModal`, the `XpubError` / `MnemonicStatus` paths) are out of
//! scope; expose `pub(crate)` test hooks on `DecryptModal` to cover
//! more.

use std::str::FromStr;
use std::sync::OnceLock;

use liana::miniscript::bitcoin::{bip32::Fingerprint, Network};

use liana_ui::widget::*;

use crate::{
    debug::{installer_with_modal, DebugMessage, DebugPageEntry},
    installer::decrypt::{decrypt_view, Decrypt, DecryptModal},
};

const BACKUP_BYTES: &[u8] = include_bytes!("../../test_assets/v0.bed");
const SOURCE_PATH: &str = "liana_gui::installer::decrypt::decrypt_view";

/// SAFETY: iced renders on the main thread; debug-overlay state is only
/// read during rendering, so satisfying `OnceLock`'s `Sync` bound with
/// an unconditional `unsafe impl Sync` is sound here.
struct StateCell<T>(T);
unsafe impl<T> Sync for StateCell<T> {}

pub static ENTRY_INITIAL: DebugPageEntry = DebugPageEntry { view: initial_view };
pub static ENTRY_OPTIONS_OPEN: DebugPageEntry = DebugPageEntry {
    view: options_open_view,
};
pub static ENTRY_MNEMONIC_NO_ACK: DebugPageEntry = DebugPageEntry {
    view: mnemonic_no_ack_view,
};
pub static ENTRY_MNEMONIC_ACKED: DebugPageEntry = DebugPageEntry {
    view: mnemonic_acked_view,
};
pub static ENTRY_FETCHED: DebugPageEntry = DebugPageEntry { view: fetched_view };
pub static ENTRY_INVALID_ENCODING: DebugPageEntry = DebugPageEntry {
    view: invalid_encoding_view,
};

fn fresh() -> DecryptModal {
    DecryptModal::new(BACKUP_BYTES.to_vec(), Network::Bitcoin)
}

fn render(state: &DecryptModal, title: &'static str) -> Element<'static, DebugMessage> {
    let body: Element<'static, _> = decrypt_view(state).into();
    installer_with_modal(title, SOURCE_PATH, body.map(|_| ()))
}

fn initial_view() -> Element<'static, DebugMessage> {
    static STATE: OnceLock<StateCell<DecryptModal>> = OnceLock::new();
    let s = STATE.get_or_init(|| StateCell(fresh()));
    render(&s.0, "Decrypt backup — initial")
}

fn options_open_view() -> Element<'static, DebugMessage> {
    static STATE: OnceLock<StateCell<DecryptModal>> = OnceLock::new();
    let s = STATE.get_or_init(|| {
        let mut m = fresh();
        let _ = m.update(Decrypt::ShowOptions(true));
        StateCell(m)
    });
    render(&s.0, "Decrypt backup — other options open")
}

fn mnemonic_no_ack_view() -> Element<'static, DebugMessage> {
    static STATE: OnceLock<StateCell<DecryptModal>> = OnceLock::new();
    let s = STATE.get_or_init(|| {
        let mut m = fresh();
        let _ = m.update(Decrypt::ShowOptions(true));
        let _ = m.update(Decrypt::SelectMnemonic);
        StateCell(m)
    });
    render(
        &s.0,
        "Decrypt backup — mnemonic input expanded, not acknowledged",
    )
}

fn mnemonic_acked_view() -> Element<'static, DebugMessage> {
    static STATE: OnceLock<StateCell<DecryptModal>> = OnceLock::new();
    let s = STATE.get_or_init(|| {
        let mut m = fresh();
        let _ = m.update(Decrypt::ShowOptions(true));
        let _ = m.update(Decrypt::SelectMnemonic);
        let _ = m.update(Decrypt::MnemonicAck(true));
        StateCell(m)
    });
    render(
        &s.0,
        "Decrypt backup — mnemonic input expanded, acknowledged",
    )
}

fn fetched_view() -> Element<'static, DebugMessage> {
    static STATE: OnceLock<StateCell<DecryptModal>> = OnceLock::new();
    let s = STATE.get_or_init(|| {
        let mut m = fresh();
        let fg = Fingerprint::from_str("8a550171").expect("valid hex");
        let _ = m.update(Decrypt::Fetched(fg, "Coldcard".to_string()));
        StateCell(m)
    });
    render(&s.0, "Decrypt backup — fetched key")
}

fn invalid_encoding_view() -> Element<'static, DebugMessage> {
    static STATE: OnceLock<StateCell<DecryptModal>> = OnceLock::new();
    let s = STATE.get_or_init(|| StateCell(DecryptModal::new(vec![], Network::Bitcoin)));
    render(&s.0, "Decrypt backup — invalid encoding error")
}

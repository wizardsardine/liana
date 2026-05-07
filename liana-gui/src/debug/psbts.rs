//! Renders rows that mirror
//! [`crate::app::view::psbts::spend_tx_list_view`]
//! (`liana-gui/src/app/view/psbts.rs:94`), with mock data covering every
//! visual variant of a PSBT row: send-to-self vs spend, signature progress,
//! recovery path, batch flag, and every [`crate::daemon::model::SpendStatus`]
//! pill (Pending / Broadcast / Spent / Deprecated).
//!
//! Construction is inlined rather than calling `spend_tx_list_view` directly:
//! the `SpendTx` it expects requires a real `PartialSpendInfo`, whose fields
//! are `pub(super)` inside the `liana` crate and not constructible from here.
//! Mirrors the "wrapped" pattern used by `debug::cards`.

use iced::{Alignment, Length};
use liana::miniscript::bitcoin::Amount;

use liana_ui::{
    component::{
        amount::{amount, amount_with_size},
        badge,
        text::*,
    },
    icon, theme,
    widget::*,
};

use crate::{
    app::menu::Menu,
    debug::{dashboard_chrome, dashboard_with_modal, DebugMessage, DebugPageEntry},
};

static MENU: Menu = Menu::PSBTs;

pub static ENTRY: DebugPageEntry = DebugPageEntry { view };

#[derive(Clone, Copy)]
enum Status {
    Pending,
    Broadcast,
    Spent,
    Deprecated,
}

#[derive(Clone, Copy)]
enum Sigs {
    Primary { signed: usize, threshold: usize },
    Recovery,
}

struct PsbtRow {
    is_send_to_self: bool,
    sigs: Sigs,
    label: Option<&'static str>,
    is_batch: bool,
    status: Status,
    spend_sats: u64,
    fee_sats: Option<u64>,
}

fn psbt_row(row: PsbtRow) -> Element<'static, DebugMessage> {
    let head_badge: Container<'static, DebugMessage> = if row.is_send_to_self {
        badge::cycle()
    } else {
        badge::spend()
    };

    let sigs_widget: Container<'static, DebugMessage> = match row.sigs {
        Sigs::Recovery => badge::recovery(),
        Sigs::Primary { signed, threshold } => {
            let displayed = signed.min(threshold);
            Container::new(
                Row::new()
                    .spacing(5)
                    .align_y(Alignment::Center)
                    .push(
                        p2_regular(format!("{displayed}/{threshold}"))
                            .style(theme::text::secondary),
                    )
                    .push(icon::key_icon().style(theme::text::secondary)),
            )
        }
    };

    let status_pill: Option<Container<'static, DebugMessage>> = match row.status {
        Status::Pending => None,
        Status::Broadcast => Some(badge::unconfirmed().width(120.0)),
        Status::Spent => Some(badge::spent().width(120.0)),
        Status::Deprecated => Some(badge::deprecated().width(120.0)),
    };

    let spend_amount = Amount::from_sat(row.spend_sats);
    let amount_col: Column<'static, DebugMessage> = Column::new()
        .align_x(Alignment::End)
        .push(if row.is_send_to_self {
            Container::new(p1_regular("Self-transfer"))
        } else {
            Container::new(amount(&spend_amount))
        })
        .push_maybe(
            row.fee_sats
                .map(|f| amount_with_size(&Amount::from_sat(f), P2_SIZE)),
        )
        .width(Length::Fixed(140.0));

    let body = Row::new()
        .spacing(20)
        .align_y(Alignment::Center)
        .push(
            Row::new()
                .spacing(10)
                .align_y(Alignment::Center)
                .push(head_badge)
                .push(sigs_widget)
                .push_maybe(row.label.map(p1_regular))
                .width(Length::Fill),
        )
        .push_maybe(row.is_batch.then(badge::batch))
        .push_maybe(status_pill)
        .push(amount_col);

    Container::new(
        Button::new(body)
            .padding(10)
            .on_press(())
            .style(theme::button::transparent_border),
    )
    .style(theme::card::button_simple)
    .into()
}

fn view() -> Element<'static, DebugMessage> {
    let rows: Vec<Element<'static, DebugMessage>> = vec![
        psbt_row(PsbtRow {
            is_send_to_self: false,
            sigs: Sigs::Primary {
                signed: 0,
                threshold: 2,
            },
            label: Some("vendor invoice"),
            is_batch: false,
            status: Status::Pending,
            spend_sats: 1_250_000,
            fee_sats: Some(2_400),
        }),
        psbt_row(PsbtRow {
            is_send_to_self: false,
            sigs: Sigs::Primary {
                signed: 1,
                threshold: 2,
            },
            label: None,
            is_batch: false,
            status: Status::Pending,
            spend_sats: 80_000,
            fee_sats: Some(900),
        }),
        psbt_row(PsbtRow {
            is_send_to_self: false,
            sigs: Sigs::Primary {
                signed: 2,
                threshold: 2,
            },
            label: Some("ready to broadcast"),
            is_batch: true,
            status: Status::Pending,
            spend_sats: 4_200_000,
            fee_sats: Some(3_500),
        }),
        psbt_row(PsbtRow {
            is_send_to_self: false,
            sigs: Sigs::Recovery,
            label: Some("emergency recovery"),
            is_batch: false,
            status: Status::Pending,
            spend_sats: 9_999_999,
            fee_sats: Some(5_000),
        }),
        psbt_row(PsbtRow {
            is_send_to_self: false,
            sigs: Sigs::Primary {
                signed: 1,
                threshold: 1,
            },
            label: Some("rent"),
            is_batch: false,
            status: Status::Broadcast,
            spend_sats: 1_500_000,
            fee_sats: Some(1_200),
        }),
        psbt_row(PsbtRow {
            is_send_to_self: false,
            sigs: Sigs::Primary {
                signed: 2,
                threshold: 2,
            },
            label: Some("supplier batch"),
            is_batch: true,
            status: Status::Spent,
            spend_sats: 25_000_000,
            fee_sats: Some(6_750),
        }),
        psbt_row(PsbtRow {
            is_send_to_self: false,
            sigs: Sigs::Primary {
                signed: 1,
                threshold: 2,
            },
            label: Some("replaced by RBF"),
            is_batch: false,
            status: Status::Deprecated,
            spend_sats: 500_000,
            fee_sats: Some(800),
        }),
        psbt_row(PsbtRow {
            is_send_to_self: true,
            sigs: Sigs::Primary {
                signed: 1,
                threshold: 2,
            },
            label: Some("vault rotation"),
            is_batch: false,
            status: Status::Pending,
            spend_sats: 0,
            fee_sats: Some(450),
        }),
    ];

    let body = rows
        .into_iter()
        .fold(Column::new().spacing(10), Column::push);
    dashboard_chrome(&MENU, "PSBTs list — variants", body)
}

// ---- import_psbt + RBF modal pages --------------------------------------

use std::sync::OnceLock;

use crate::app::view::psbts::{import_psbt_success_view, import_psbt_view};
use crate::app::view::transactions::create_rbf_modal;
use liana::miniscript::bitcoin::Txid;
use std::collections::HashSet;
use std::str::FromStr;

pub static ENTRY_IMPORT_EMPTY: DebugPageEntry = DebugPageEntry {
    view: render_import_empty,
};
pub static ENTRY_IMPORT_TYPED: DebugPageEntry = DebugPageEntry {
    view: render_import_typed,
};
pub static ENTRY_IMPORT_PROCESSING: DebugPageEntry = DebugPageEntry {
    view: render_import_processing,
};
pub static ENTRY_IMPORT_SUCCESS: DebugPageEntry = DebugPageEntry {
    view: render_import_success,
};
pub static ENTRY_RBF_BUMP: DebugPageEntry = DebugPageEntry {
    view: render_rbf_bump,
};
pub static ENTRY_RBF_REPLACED: DebugPageEntry = DebugPageEntry {
    view: render_rbf_replaced,
};
pub static ENTRY_RBF_CANCEL: DebugPageEntry = DebugPageEntry {
    view: render_rbf_cancel,
};

fn empty_psbt_form() -> &'static liana_ui::component::form::Value<String> {
    static F: OnceLock<liana_ui::component::form::Value<String>> = OnceLock::new();
    F.get_or_init(liana_ui::component::form::Value::default)
}

fn typed_psbt_form() -> &'static liana_ui::component::form::Value<String> {
    static F: OnceLock<liana_ui::component::form::Value<String>> = OnceLock::new();
    F.get_or_init(|| liana_ui::component::form::Value {
        value: "cHNidP8BAH0CAAAAAYYd…(truncated for display)".to_string(),
        warning: None,
        valid: true,
    })
}

fn render_import_empty() -> Element<'static, DebugMessage> {
    let body = import_psbt_view(empty_psbt_form(), None, false).map(|_| ());
    dashboard_with_modal(&MENU, "PSBT — import (empty form)", body)
}

fn render_import_typed() -> Element<'static, DebugMessage> {
    let body = import_psbt_view(typed_psbt_form(), None, false).map(|_| ());
    dashboard_with_modal(&MENU, "PSBT — import (typed)", body)
}

fn render_import_processing() -> Element<'static, DebugMessage> {
    let body = import_psbt_view(typed_psbt_form(), None, true).map(|_| ());
    dashboard_with_modal(&MENU, "PSBT — import (processing)", body)
}

fn render_import_success() -> Element<'static, DebugMessage> {
    let body = import_psbt_success_view().map(|_| ());
    dashboard_with_modal(&MENU, "PSBT — import (success)", body)
}

fn empty_descendants() -> &'static HashSet<Txid> {
    static S: OnceLock<HashSet<Txid>> = OnceLock::new();
    S.get_or_init(HashSet::new)
}

fn feerate_form() -> &'static liana_ui::component::form::Value<String> {
    static F: OnceLock<liana_ui::component::form::Value<String>> = OnceLock::new();
    F.get_or_init(|| liana_ui::component::form::Value {
        value: "12".to_string(),
        warning: None,
        valid: true,
    })
}

fn replacement_txid() -> Txid {
    Txid::from_str(&"a".repeat(64)).expect("valid hex")
}

fn render_rbf_bump() -> Element<'static, DebugMessage> {
    let body = create_rbf_modal(false, empty_descendants(), feerate_form(), None, None).map(|_| ());
    dashboard_with_modal(&MENU, "PSBT — RBF (fee bump form)", body)
}

fn render_rbf_replaced() -> Element<'static, DebugMessage> {
    let body = create_rbf_modal(
        false,
        empty_descendants(),
        feerate_form(),
        Some(replacement_txid()),
        None,
    )
    .map(|_| ());
    dashboard_with_modal(&MENU, "PSBT — RBF (replacement created)", body)
}

fn render_rbf_cancel() -> Element<'static, DebugMessage> {
    let body = create_rbf_modal(true, empty_descendants(), feerate_form(), None, None).map(|_| ());
    dashboard_with_modal(&MENU, "PSBT — RBF (cancel transaction)", body)
}

// ---- psbt_view (single PSBT detail) -------------------------------------

use liana::descriptors::{LianaDescriptor, LianaPolicy, PartialSpendInfo, PathSpendInfo};
use liana::miniscript::bitcoin::absolute::LockTime;
use liana::miniscript::bitcoin::bip32::Fingerprint;
use liana::miniscript::bitcoin::psbt::{Input as PsbtInput, Output as PsbtOutput, Psbt};
use liana::miniscript::bitcoin::transaction::Version as TxVersion;
use liana::miniscript::bitcoin::{
    Network, OutPoint, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Witness,
};
use std::collections::BTreeMap;
use std::collections::HashMap;

use crate::app::view::psbt::psbt_view;
use crate::daemon::model::{SpendStatus, SpendTx, TransactionKind};

const SAMPLE_DESCRIPTOR: &str = "wsh(or_d(pk([19608592/48'/1'/0'/2']tpubDEjf1AbrUjxnw8jg6Gi12CunPqnCobLP6Ktoy4Hd52pa65d6QRPg5CSkdFrqPDjJ8BAUuMEDVDRQVjtuWWksMqBeZCqyABFucN9ErQq8oVX/<0;1>/*),and_v(v:pkh([19608592/48'/1'/0'/2']tpubDEjf1AbrUjxnw8jg6Gi12CunPqnCobLP6Ktoy4Hd52pa65d6QRPg5CSkdFrqPDjJ8BAUuMEDVDRQVjtuWWksMqBeZCqyABFucN9ErQq8oVX/<2;3>/*),older(52596))))#x6u6lmej";

/// SAFETY: iced renders on the main thread.
struct PsbtsCell<T>(T);
unsafe impl<T> Sync for PsbtsCell<T> {}

fn sample_descriptor() -> &'static LianaDescriptor {
    static D: OnceLock<LianaDescriptor> = OnceLock::new();
    D.get_or_init(|| {
        use std::str::FromStr;
        LianaDescriptor::from_str(SAMPLE_DESCRIPTOR).expect("sample descriptor parses")
    })
}

fn sample_policy() -> &'static LianaPolicy {
    static P: OnceLock<LianaPolicy> = OnceLock::new();
    P.get_or_init(|| sample_descriptor().policy())
}

fn empty_aliases() -> &'static HashMap<Fingerprint, String> {
    static M: OnceLock<HashMap<Fingerprint, String>> = OnceLock::new();
    M.get_or_init(HashMap::new)
}

fn empty_label_forms_p() -> &'static HashMap<String, liana_ui::component::form::Value<String>> {
    static M: OnceLock<HashMap<String, liana_ui::component::form::Value<String>>> = OnceLock::new();
    M.get_or_init(HashMap::new)
}

fn build_minimal_psbt(out_value: Amount) -> Psbt {
    let prev = OutPoint {
        txid: Txid::from_str(&"0".repeat(64)).unwrap(),
        vout: 0,
    };
    let tx = Transaction {
        version: TxVersion(2),
        lock_time: LockTime::ZERO,
        input: vec![TxIn {
            previous_output: prev,
            script_sig: ScriptBuf::new(),
            sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
            witness: Witness::new(),
        }],
        output: vec![TxOut {
            value: out_value,
            script_pubkey: ScriptBuf::new(),
        }],
    };
    Psbt {
        unsigned_tx: tx,
        version: 0,
        xpub: BTreeMap::new(),
        proprietary: BTreeMap::new(),
        unknown: BTreeMap::new(),
        inputs: vec![PsbtInput::default()],
        outputs: vec![PsbtOutput::default()],
    }
}

fn make_spend_tx(
    status: SpendStatus,
    primary_signed: usize,
    primary_threshold: usize,
    has_recovery: bool,
) -> SpendTx {
    let mut recovery_paths = BTreeMap::new();
    if has_recovery {
        recovery_paths.insert(
            52_596,
            PathSpendInfo {
                threshold: 1,
                sigs_count: 1,
                signed_pubkeys: HashMap::new(),
            },
        );
    }
    let sigs = PartialSpendInfo::new(
        PathSpendInfo {
            threshold: primary_threshold,
            sigs_count: primary_signed,
            signed_pubkeys: HashMap::new(),
        },
        recovery_paths,
    );
    let psbt = build_minimal_psbt(Amount::from_sat(1_500_000));
    let outpoint = OutPoint {
        txid: psbt.unsigned_tx.compute_txid(),
        vout: 0,
    };
    SpendTx {
        network: Network::Bitcoin,
        coins: HashMap::new(),
        labels: HashMap::new(),
        psbt,
        change_indexes: Vec::new(),
        spend_amount: Amount::from_sat(1_500_000),
        fee_amount: Some(Amount::from_sat(2_400)),
        max_vbytes: 250,
        status,
        sigs,
        updated_at: Some(1_700_000_000),
        kind: TransactionKind::OutgoingSinglePayment(outpoint),
    }
}

fn pending_spend_tx() -> &'static SpendTx {
    static T: OnceLock<PsbtsCell<SpendTx>> = OnceLock::new();
    &T.get_or_init(|| PsbtsCell(make_spend_tx(SpendStatus::Pending, 1, 2, false)))
        .0
}

/// Public wrapper so other debug modules (panels.rs Send pages) can
/// reuse the same mock SpendTx.
pub(super) fn pending_spend_tx_pub() -> &'static SpendTx {
    pending_spend_tx()
}

pub(super) fn broadcast_spend_tx_pub() -> &'static SpendTx {
    broadcast_spend_tx()
}

pub(super) fn sample_policy_pub() -> &'static LianaPolicy {
    sample_policy()
}

fn broadcast_spend_tx() -> &'static SpendTx {
    static T: OnceLock<PsbtsCell<SpendTx>> = OnceLock::new();
    &T.get_or_init(|| PsbtsCell(make_spend_tx(SpendStatus::Broadcast, 2, 2, false)))
        .0
}

fn spent_spend_tx() -> &'static SpendTx {
    static T: OnceLock<PsbtsCell<SpendTx>> = OnceLock::new();
    &T.get_or_init(|| PsbtsCell(make_spend_tx(SpendStatus::Spent, 2, 2, false)))
        .0
}

fn recovery_spend_tx() -> &'static SpendTx {
    static T: OnceLock<PsbtsCell<SpendTx>> = OnceLock::new();
    &T.get_or_init(|| PsbtsCell(make_spend_tx(SpendStatus::Pending, 0, 2, true)))
        .0
}

pub static ENTRY_PSBT_PENDING: DebugPageEntry = DebugPageEntry {
    view: render_psbt_pending,
};
pub static ENTRY_PSBT_BROADCAST: DebugPageEntry = DebugPageEntry {
    view: render_psbt_broadcast,
};
pub static ENTRY_PSBT_SPENT: DebugPageEntry = DebugPageEntry {
    view: render_psbt_spent,
};
pub static ENTRY_PSBT_RECOVERY: DebugPageEntry = DebugPageEntry {
    view: render_psbt_recovery,
};

fn render_psbt_pending() -> Element<'static, DebugMessage> {
    psbt_view(
        crate::debug::static_cache(),
        pending_spend_tx(),
        false,
        sample_policy(),
        empty_aliases(),
        empty_label_forms_p(),
        Network::Bitcoin,
        false,
        None,
    )
    .map(|_| ())
}

fn render_psbt_broadcast() -> Element<'static, DebugMessage> {
    psbt_view(
        crate::debug::static_cache(),
        broadcast_spend_tx(),
        true,
        sample_policy(),
        empty_aliases(),
        empty_label_forms_p(),
        Network::Bitcoin,
        false,
        None,
    )
    .map(|_| ())
}

fn render_psbt_spent() -> Element<'static, DebugMessage> {
    psbt_view(
        crate::debug::static_cache(),
        spent_spend_tx(),
        true,
        sample_policy(),
        empty_aliases(),
        empty_label_forms_p(),
        Network::Bitcoin,
        false,
        None,
    )
    .map(|_| ())
}

fn render_psbt_recovery() -> Element<'static, DebugMessage> {
    psbt_view(
        crate::debug::static_cache(),
        recovery_spend_tx(),
        false,
        sample_policy(),
        empty_aliases(),
        empty_label_forms_p(),
        Network::Bitcoin,
        false,
        None,
    )
    .map(|_| ())
}

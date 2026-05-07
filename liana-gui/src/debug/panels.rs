//! Debug pages for the main wallet panel views (Coins, Receive,
//! Recovery). Most of these views return inner content (no internal
//! `dashboard()` wrap), so we wrap them with [`crate::debug::dashboard_chrome`]
//! to get the sidebar-correct rendering. `recovery::recovery` is the
//! exception — it wraps in `dashboard()` itself, so we render it
//! straight through `.map(|_| ())`.

use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::sync::OnceLock;

use liana::miniscript::bitcoin::{bip32::ChildNumber, Address, Amount, Network, OutPoint, Txid};
use liana_ui::widget::Element;

use crate::{
    app::{
        menu::Menu,
        view::{coins as coins_view_mod, receive as receive_view, recovery as recovery_view},
    },
    daemon::model::Coin,
    debug::{
        dashboard_chrome, dashboard_with_modal, DebugMessage, DebugPageEntry, COINS_MENU,
        RECEIVE_MENU, RECOVERY_MENU,
    },
};

/// Sample Bitcoin addresses used by the receive / coins debug pages.
/// They are valid mainnet bech32 — nothing in the rendering path
/// validates ownership.
const SAMPLE_ADDRESSES: &[&str] = &[
    "bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq",
    "bc1q9d4ywgfnd8h43da5tpcxcn6ajv590cg6d3tg6axemvljvt2k76zs50tv4q",
    "bc1pmfr3p9j00pfxjh0zmgp99y8zftmd3s5pmedqhyptwy6lm87hf5sspknck9",
    "bc1qrp33g0q5c5txsp9arysrx4k6zdkfs4nce4xj0gdcccefvpysxf3qccfmv3",
];

fn parse_addr(s: &str) -> Address {
    Address::from_str(s)
        .expect("hardcoded sample")
        .require_network(Network::Bitcoin)
        .expect("mainnet sample")
}

// ---- Coins ----------------------------------------------------------------

fn sample_coins() -> &'static Vec<Coin> {
    static COINS: OnceLock<Vec<Coin>> = OnceLock::new();
    COINS.get_or_init(|| {
        let txid = Txid::from_str("0".repeat(64).as_str()).expect("valid hex");
        vec![
            Coin {
                amount: Amount::from_sat(2_500_000),
                outpoint: OutPoint { txid, vout: 0 },
                address: parse_addr(SAMPLE_ADDRESSES[0]),
                block_height: Some(800_000),
                derivation_index: ChildNumber::from_normal_idx(0).unwrap(),
                spend_info: None,
                is_immature: false,
                is_change: false,
                is_from_self: false,
            },
            Coin {
                amount: Amount::from_sat(50_000),
                outpoint: OutPoint { txid, vout: 1 },
                address: parse_addr(SAMPLE_ADDRESSES[1]),
                block_height: None, // unconfirmed
                derivation_index: ChildNumber::from_normal_idx(1).unwrap(),
                spend_info: None,
                is_immature: false,
                is_change: false,
                is_from_self: false,
            },
            Coin {
                amount: Amount::from_sat(10_000_000),
                outpoint: OutPoint { txid, vout: 2 },
                address: parse_addr(SAMPLE_ADDRESSES[2]),
                block_height: Some(799_500), // older — closer to expiry
                derivation_index: ChildNumber::from_normal_idx(2).unwrap(),
                spend_info: None,
                is_immature: false,
                is_change: false,
                is_from_self: true,
            },
        ]
    })
}

fn empty_labels() -> &'static HashMap<String, String> {
    static M: OnceLock<HashMap<String, String>> = OnceLock::new();
    M.get_or_init(HashMap::new)
}

fn empty_label_forms() -> &'static HashMap<String, liana_ui::component::form::Value<String>> {
    static M: OnceLock<HashMap<String, liana_ui::component::form::Value<String>>> = OnceLock::new();
    M.get_or_init(HashMap::new)
}

pub static ENTRY_COINS_EMPTY: DebugPageEntry = DebugPageEntry {
    view: render_coins_empty,
};
pub static ENTRY_COINS_WITH_DATA: DebugPageEntry = DebugPageEntry {
    view: render_coins_with_data,
};

fn render_coins_empty() -> Element<'static, DebugMessage> {
    let body = coins_view_mod::coins_view(
        crate::debug::static_cache(),
        &[],
        4_032,
        &[],
        empty_labels(),
        empty_label_forms(),
    )
    .map(|_| ());
    dashboard_chrome(&COINS_MENU, "Coins panel — empty", body)
}

fn render_coins_with_data() -> Element<'static, DebugMessage> {
    let body = coins_view_mod::coins_view(
        crate::debug::static_cache(),
        sample_coins(),
        4_032, // ~4 weeks of blocks
        &[],
        empty_labels(),
        empty_label_forms(),
    )
    .map(|_| ());
    dashboard_chrome(&COINS_MENU, "Coins panel — with coins", body)
}

// ---- Receive --------------------------------------------------------------

fn sample_addresses() -> &'static Vec<Address> {
    static A: OnceLock<Vec<Address>> = OnceLock::new();
    A.get_or_init(|| SAMPLE_ADDRESSES.iter().copied().map(parse_addr).collect())
}

fn empty_address_set() -> &'static HashSet<Address> {
    static S: OnceLock<HashSet<Address>> = OnceLock::new();
    S.get_or_init(HashSet::new)
}

pub static ENTRY_RECEIVE: DebugPageEntry = DebugPageEntry {
    view: render_receive,
};
pub static ENTRY_RECEIVE_WITH_PREV: DebugPageEntry = DebugPageEntry {
    view: render_receive_with_prev,
};

fn render_receive() -> Element<'static, DebugMessage> {
    let addrs = sample_addresses();
    let body = receive_view::receive(
        &addrs[..1],
        empty_labels(),
        &[],
        empty_labels(),
        false,
        empty_address_set(),
        empty_label_forms(),
        true,
        false,
    )
    .map(|_| ());
    dashboard_chrome(&RECEIVE_MENU, "Receive panel — single fresh address", body)
}

fn render_receive_with_prev() -> Element<'static, DebugMessage> {
    let addrs = sample_addresses();
    let body = receive_view::receive(
        &addrs[..1],
        empty_labels(),
        &addrs[1..],
        empty_labels(),
        true,
        empty_address_set(),
        empty_label_forms(),
        true,
        false,
    )
    .map(|_| ());
    dashboard_chrome(
        &RECEIVE_MENU,
        "Receive panel — with previous addresses shown",
        body,
    )
}

// ---- Recovery -------------------------------------------------------------

pub static ENTRY_RECOVERY_NONE: DebugPageEntry = DebugPageEntry {
    view: render_recovery_none,
};

fn render_recovery_none() -> Element<'static, DebugMessage> {
    // recovery::recovery wraps in `dashboard(...)` internally, so we
    // render straight through `.map(|_| ())` without re-wrapping.
    let _ = (RECOVERY_MENU.clone(), Menu::Recovery); // suppress unused warning if applicable
    recovery_view::recovery(crate::debug::static_cache(), Vec::new(), None, None).map(|_| ())
}

// ---- Transactions: tx_view (single tx detail) ----------------------------

use iced::widget::qr_code;
use liana::miniscript::bitcoin::absolute::LockTime;
use liana::miniscript::bitcoin::transaction::Version;
use liana::miniscript::bitcoin::{ScriptBuf, Sequence, Transaction, TxIn, TxOut, Witness};

use crate::app::view::{receive::qr_modal, receive::verify_address_modal, transactions::tx_view};
use crate::daemon::model::{HistoryTransaction, TransactionKind};
use crate::debug::{hw_modals, TRANSACTIONS_MENU};
use async_hwi::DeviceKind;
use std::collections::HashSet as StdHashSet;

/// SAFETY: iced renders on the main thread.
struct PanelsCell<T>(T);
unsafe impl<T> Sync for PanelsCell<T> {}

fn dummy_tx(out_value: Amount) -> Transaction {
    Transaction {
        version: Version(2),
        lock_time: LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint {
                txid: Txid::from_str(&"0".repeat(64)).unwrap(),
                vout: 0,
            },
            script_sig: ScriptBuf::new(),
            sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
            witness: Witness::new(),
        }],
        output: vec![TxOut {
            value: out_value,
            script_pubkey: parse_addr(SAMPLE_ADDRESSES[0]).script_pubkey(),
        }],
    }
}

fn build_history_tx(
    kind: TransactionKind,
    out_value: Amount,
    time: Option<u32>,
    fee: Option<Amount>,
) -> HistoryTransaction {
    let tx = dummy_tx(out_value);
    let txid = tx.compute_txid();
    HistoryTransaction {
        network: Network::Bitcoin,
        labels: HashMap::new(),
        coins: HashMap::new(),
        change_indexes: Vec::new(),
        tx,
        txid,
        outgoing_amount: match &kind {
            TransactionKind::OutgoingSinglePayment(_)
            | TransactionKind::OutgoingPaymentBatch(_) => out_value,
            _ => Amount::ZERO,
        },
        incoming_amount: match &kind {
            TransactionKind::IncomingSinglePayment(_)
            | TransactionKind::IncomingPaymentBatch(_) => out_value,
            _ => Amount::ZERO,
        },
        fee_amount: fee,
        height: time.map(|_| 800_000),
        time,
        kind,
    }
}

fn outgoing_confirmed_tx() -> &'static HistoryTransaction {
    static T: OnceLock<HistoryTransaction> = OnceLock::new();
    T.get_or_init(|| {
        let outpoint = OutPoint {
            txid: Txid::from_str(&"a".repeat(64)).unwrap(),
            vout: 0,
        };
        build_history_tx(
            TransactionKind::OutgoingSinglePayment(outpoint),
            Amount::from_sat(1_500_000),
            Some(1_700_000_000),
            Some(Amount::from_sat(2_400)),
        )
    })
}

fn incoming_unconfirmed_tx() -> &'static HistoryTransaction {
    static T: OnceLock<HistoryTransaction> = OnceLock::new();
    T.get_or_init(|| {
        let outpoint = OutPoint {
            txid: Txid::from_str(&"b".repeat(64)).unwrap(),
            vout: 0,
        };
        build_history_tx(
            TransactionKind::IncomingSinglePayment(outpoint),
            Amount::from_sat(250_000),
            None,
            None,
        )
    })
}

fn self_transfer_tx() -> &'static HistoryTransaction {
    static T: OnceLock<HistoryTransaction> = OnceLock::new();
    T.get_or_init(|| {
        build_history_tx(
            TransactionKind::SendToSelf,
            Amount::from_sat(0),
            Some(1_700_000_000),
            Some(Amount::from_sat(800)),
        )
    })
}

pub static ENTRY_TX_OUTGOING: DebugPageEntry = DebugPageEntry {
    view: render_tx_outgoing,
};
pub static ENTRY_TX_INCOMING: DebugPageEntry = DebugPageEntry {
    view: render_tx_incoming,
};
pub static ENTRY_TX_SELF: DebugPageEntry = DebugPageEntry {
    view: render_tx_self,
};

fn render_tx_outgoing() -> Element<'static, DebugMessage> {
    tx_view(
        crate::debug::static_cache(),
        outgoing_confirmed_tx(),
        empty_label_forms(),
        None,
    )
    .map(|_| ())
}

fn render_tx_incoming() -> Element<'static, DebugMessage> {
    tx_view(
        crate::debug::static_cache(),
        incoming_unconfirmed_tx(),
        empty_label_forms(),
        None,
    )
    .map(|_| ())
}

fn render_tx_self() -> Element<'static, DebugMessage> {
    tx_view(
        crate::debug::static_cache(),
        self_transfer_tx(),
        empty_label_forms(),
        None,
    )
    .map(|_| ())
}

// ---- Receive: verify_address_modal + qr_modal ----------------------------

fn verify_address_hws() -> &'static Vec<crate::hw::HardwareWallet> {
    static V: OnceLock<Vec<crate::hw::HardwareWallet>> = OnceLock::new();
    V.get_or_init(|| {
        vec![
            hw_modals::supported(
                DeviceKind::Ledger,
                Some(hw_modals::ver(2, 1, 0)),
                hw_modals::fp(0xAA),
                Some("Vault key"),
                Some(true),
            ),
            hw_modals::supported(
                DeviceKind::BitBox02,
                Some(hw_modals::ver(9, 13, 0)),
                hw_modals::fp(0xBB),
                Some("Backup key"),
                Some(true),
            ),
        ]
    })
}

fn empty_chosen_set() -> &'static StdHashSet<liana::miniscript::bitcoin::bip32::Fingerprint> {
    static S: OnceLock<StdHashSet<liana::miniscript::bitcoin::bip32::Fingerprint>> =
        OnceLock::new();
    S.get_or_init(StdHashSet::new)
}

fn first_address() -> &'static Address {
    static A: OnceLock<Address> = OnceLock::new();
    A.get_or_init(|| parse_addr(SAMPLE_ADDRESSES[0]))
}

fn first_index() -> &'static ChildNumber {
    static I: OnceLock<ChildNumber> = OnceLock::new();
    I.get_or_init(|| ChildNumber::from_normal_idx(0).unwrap())
}

pub static ENTRY_VERIFY_ADDRESS: DebugPageEntry = DebugPageEntry {
    view: render_verify_address,
};

fn render_verify_address() -> Element<'static, DebugMessage> {
    let body = verify_address_modal(
        None,
        verify_address_hws(),
        empty_chosen_set(),
        first_address(),
        first_index(),
    )
    .map(|_| ());
    dashboard_with_modal(&RECEIVE_MENU, "Receive — verify address modal", body)
}

fn qr_data() -> &'static qr_code::Data {
    static Q: OnceLock<PanelsCell<qr_code::Data>> = OnceLock::new();
    &Q.get_or_init(|| {
        PanelsCell(
            qr_code::Data::new(format!("bitcoin:{}", SAMPLE_ADDRESSES[0]))
                .expect("qr-encoded sample uri"),
        )
    })
    .0
}

fn qr_address_string() -> &'static String {
    static A: OnceLock<String> = OnceLock::new();
    A.get_or_init(|| SAMPLE_ADDRESSES[0].to_string())
}

pub static ENTRY_QR_MODAL: DebugPageEntry = DebugPageEntry {
    view: render_qr_modal,
};

fn render_qr_modal() -> Element<'static, DebugMessage> {
    let body = qr_modal(qr_data(), qr_address_string()).map(|_| ());
    dashboard_with_modal(&RECEIVE_MENU, "Receive — QR modal", body)
}

// Reference to TRANSACTIONS_MENU to avoid unused-import warning when that
// stack registration moves elsewhere.
#[allow(dead_code)]
fn _keep_transactions_menu_import() -> &'static Menu {
    &TRANSACTIONS_MENU
}

// ---- Send panel: spend_view ---------------------------------------------

use crate::app::view::spend::spend_view;
use crate::debug::SEND_MENU;

pub static ENTRY_SPEND_DRAFTING: DebugPageEntry = DebugPageEntry {
    view: render_spend_drafting,
};
pub static ENTRY_SPEND_REVIEWING: DebugPageEntry = DebugPageEntry {
    view: render_spend_reviewing,
};

// `spend_view` and `create_spend_tx` internally call `dashboard(...)`,
// so we render them straight through `.map(|_| ())` — wrapping with
// `dashboard_chrome` would double-nest the responsive() callback and
// cause visible flicker as both writers fight over `Cache.pane_size`.

fn render_spend_drafting() -> Element<'static, DebugMessage> {
    spend_view(
        crate::debug::static_cache(),
        crate::debug::psbts::pending_spend_tx_pub(),
        &[],
        false,
        crate::debug::psbts::sample_policy_pub(),
        empty_aliases_pub(),
        empty_label_forms(),
        Network::Bitcoin,
        false,
        None,
    )
    .map(|_| ())
}

fn render_spend_reviewing() -> Element<'static, DebugMessage> {
    spend_view(
        crate::debug::static_cache(),
        crate::debug::psbts::broadcast_spend_tx_pub(),
        &[],
        true,
        crate::debug::psbts::sample_policy_pub(),
        empty_aliases_pub(),
        empty_label_forms(),
        Network::Bitcoin,
        false,
        None,
    )
    .map(|_| ())
}

fn empty_aliases_pub() -> &'static HashMap<liana::miniscript::bitcoin::bip32::Fingerprint, String> {
    static M: OnceLock<HashMap<liana::miniscript::bitcoin::bip32::Fingerprint, String>> =
        OnceLock::new();
    M.get_or_init(HashMap::new)
}

// ---- create_spend_tx + recipient_view ------------------------------------

use crate::app::view::spend::{create_spend_tx, recipient_view};

fn empty_str_form() -> &'static liana_ui::component::form::Value<String> {
    static F: OnceLock<liana_ui::component::form::Value<String>> = OnceLock::new();
    F.get_or_init(liana_ui::component::form::Value::default)
}

fn typed_str_form(s: &str) -> liana_ui::component::form::Value<String> {
    liana_ui::component::form::Value {
        value: s.to_string(),
        warning: None,
        valid: true,
    }
}

pub static ENTRY_RECIPIENT_VALID: DebugPageEntry = DebugPageEntry {
    view: render_recipient_valid,
};
pub static ENTRY_CREATE_SPEND_SELF: DebugPageEntry = DebugPageEntry {
    view: render_create_spend_self,
};
pub static ENTRY_CREATE_SPEND_FILLED: DebugPageEntry = DebugPageEntry {
    view: render_create_spend_filled,
};

fn recipient_addr_form() -> &'static liana_ui::component::form::Value<String> {
    static F: OnceLock<liana_ui::component::form::Value<String>> = OnceLock::new();
    F.get_or_init(|| typed_str_form(SAMPLE_ADDRESSES[0]))
}

fn recipient_amount_form() -> &'static liana_ui::component::form::Value<String> {
    static F: OnceLock<liana_ui::component::form::Value<String>> = OnceLock::new();
    F.get_or_init(|| typed_str_form("0.05"))
}

fn render_recipient_valid() -> Element<'static, DebugMessage> {
    // recipient_view returns Element<CreateSpendMessage>. Map twice to
    // get to DebugMessage (CreateSpendMessage → () → ()).
    let body: Element<'static, ()> = recipient_view(
        0,
        recipient_addr_form(),
        recipient_amount_form(),
        None,
        None,
        empty_str_form(),
        false,
        false,
        &None,
        None,
    )
    .map(|_| ());
    dashboard_chrome(&SEND_MENU, "Send — single recipient (valid form)", body)
}

fn render_create_spend_self() -> Element<'static, DebugMessage> {
    create_spend_tx(
        crate::debug::static_cache(),
        None,
        Vec::new(),
        false,
        false,
        4_032,
        None,
        &[],
        empty_labels(),
        empty_str_form(),
        None,
        recipient_amount_form(),
        None,
        None,
        true,
        false,
    )
    .map(|_| ())
}

fn render_create_spend_filled() -> Element<'static, DebugMessage> {
    let recipient = recipient_view(
        0,
        recipient_addr_form(),
        recipient_amount_form(),
        None,
        None,
        empty_str_form(),
        false,
        false,
        &None,
        None,
    )
    .map(crate::app::view::Message::CreateSpend);
    create_spend_tx(
        crate::debug::static_cache(),
        None,
        vec![recipient],
        true,
        false,
        4_032,
        None,
        &[],
        empty_labels(),
        empty_str_form(),
        None,
        recipient_amount_form(),
        None,
        None,
        false,
        false,
    )
    .map(|_| ())
}

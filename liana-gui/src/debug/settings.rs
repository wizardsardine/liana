//! Debug pages for the production Settings panel views in
//! `crate::app::view::settings::*`. Most settings views internally call
//! `view::dashboard(...)`, so the page renders the production output
//! straight through `.map(|_| ())` without re-wrapping in
//! [`crate::debug::dashboard_chrome`] (that would double-nest the
//! sidebar).

use std::collections::HashMap;
use std::collections::HashSet;
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::OnceLock;

use async_hwi::DeviceKind;
use liana::descriptors::LianaDescriptor;
use liana::miniscript::bitcoin::{bip32::Fingerprint, Network};
use liana_ui::widget::Element;
use lianad::config::{BitcoindConfig, BitcoindRpcAuth, ElectrumConfig};

use crate::{
    app::{settings::fiat::PriceSetting, view::settings as v},
    debug::{
        dashboard_chrome, dashboard_with_modal, hw_modals, static_cache, DebugMessage,
        DebugPageEntry, SETTINGS_MENU,
    },
    hw::HardwareWallet,
    node::bitcoind::{RpcAuthType, RpcAuthValues},
    services::fiat::Currency,
};

pub static ENTRY_LIST_LOCAL: DebugPageEntry = DebugPageEntry {
    view: render_list_local,
};
pub static ENTRY_LIST_REMOTE: DebugPageEntry = DebugPageEntry {
    view: render_list_remote,
};
pub static ENTRY_ABOUT: DebugPageEntry = DebugPageEntry { view: render_about };
pub static ENTRY_IMPORT_EXPORT: DebugPageEntry = DebugPageEntry {
    view: render_import_export,
};
pub static ENTRY_GENERAL_OFF: DebugPageEntry = DebugPageEntry {
    view: render_general_off,
};
pub static ENTRY_GENERAL_ON: DebugPageEntry = DebugPageEntry {
    view: render_general_on,
};
pub static ENTRY_REMOTE_BACKEND_IDLE: DebugPageEntry = DebugPageEntry {
    view: render_remote_backend_idle,
};
pub static ENTRY_REMOTE_BACKEND_PROCESSING: DebugPageEntry = DebugPageEntry {
    view: render_remote_backend_processing,
};
pub static ENTRY_REMOTE_BACKEND_SUCCESS: DebugPageEntry = DebugPageEntry {
    view: render_remote_backend_success,
};

fn render_list_local() -> Element<'static, DebugMessage> {
    v::list(static_cache(), false).map(|_| ())
}

fn render_list_remote() -> Element<'static, DebugMessage> {
    v::list(static_cache(), true).map(|_| ())
}

fn render_about() -> Element<'static, DebugMessage> {
    static VERSION: OnceLock<String> = OnceLock::new();
    let v_str = VERSION.get_or_init(|| "1.0.0".to_string());
    v::about_section(static_cache(), None, Some(v_str)).map(|_| ())
}

fn render_import_export() -> Element<'static, DebugMessage> {
    v::import_export(static_cache(), None).map(|_| ())
}

const ALL_CURRENCIES: &[Currency] = &[Currency::USD, Currency::EUR];

fn fiat_off_setting() -> &'static PriceSetting {
    static S: OnceLock<PriceSetting> = OnceLock::new();
    S.get_or_init(PriceSetting::default)
}

fn fiat_on_setting() -> &'static PriceSetting {
    static S: OnceLock<PriceSetting> = OnceLock::new();
    S.get_or_init(|| PriceSetting {
        is_enabled: true,
        currency: Currency::USD,
        ..PriceSetting::default()
    })
}

fn render_general_off() -> Element<'static, DebugMessage> {
    v::general::general_section(static_cache(), fiat_off_setting(), ALL_CURRENCIES, None)
        .map(|_| ())
}

fn render_general_on() -> Element<'static, DebugMessage> {
    v::general::general_section(static_cache(), fiat_on_setting(), ALL_CURRENCIES, None).map(|_| ())
}

// ---- remote backend section --------------------------------------

fn empty_email_form() -> &'static liana_ui::component::form::Value<String> {
    static F: OnceLock<liana_ui::component::form::Value<String>> = OnceLock::new();
    F.get_or_init(liana_ui::component::form::Value::default)
}

fn typed_email_form() -> &'static liana_ui::component::form::Value<String> {
    static F: OnceLock<liana_ui::component::form::Value<String>> = OnceLock::new();
    F.get_or_init(|| liana_ui::component::form::Value {
        value: "alice@example.com".to_string(),
        warning: None,
        valid: true,
    })
}

fn render_remote_backend_idle() -> Element<'static, DebugMessage> {
    v::remote_backend_section(static_cache(), empty_email_form(), false, false, None).map(|_| ())
}

fn render_remote_backend_processing() -> Element<'static, DebugMessage> {
    v::remote_backend_section(static_cache(), typed_email_form(), true, false, None).map(|_| ())
}

fn render_remote_backend_success() -> Element<'static, DebugMessage> {
    v::remote_backend_section(static_cache(), typed_email_form(), false, true, None).map(|_| ())
}

// ---- bitcoind / electrum read-only views (wrap themselves in dashboard) ---

fn bitcoind_config() -> &'static BitcoindConfig {
    static C: OnceLock<BitcoindConfig> = OnceLock::new();
    C.get_or_init(|| BitcoindConfig {
        rpc_auth: BitcoindRpcAuth::UserPass("rpcuser".to_string(), "rpcpass".to_string()),
        addr: SocketAddr::from(([127, 0, 0, 1], 8332)),
    })
}

fn electrum_config() -> &'static ElectrumConfig {
    static C: OnceLock<ElectrumConfig> = OnceLock::new();
    C.get_or_init(|| ElectrumConfig {
        addr: "ssl://electrum.blockstream.info:50002".to_string(),
        validate_domain: true,
    })
}

pub static ENTRY_BITCOIND_RUNNING: DebugPageEntry = DebugPageEntry {
    view: render_bitcoind_running,
};
pub static ENTRY_BITCOIND_STOPPED: DebugPageEntry = DebugPageEntry {
    view: render_bitcoind_stopped,
};
pub static ENTRY_BITCOIND_EDIT: DebugPageEntry = DebugPageEntry {
    view: render_bitcoind_edit,
};
pub static ENTRY_ELECTRUM_RUNNING: DebugPageEntry = DebugPageEntry {
    view: render_electrum_running,
};
pub static ENTRY_ELECTRUM_EDIT: DebugPageEntry = DebugPageEntry {
    view: render_electrum_edit,
};

fn render_bitcoind_running() -> Element<'static, DebugMessage> {
    let inner = v::bitcoind(
        true,
        Network::Bitcoin,
        bitcoind_config(),
        800_000,
        Some(true),
        true,
    )
    .map(|_| ());
    dashboard_chrome(&SETTINGS_MENU, "Settings — bitcoind (running)", inner)
}

fn render_bitcoind_stopped() -> Element<'static, DebugMessage> {
    let inner = v::bitcoind(
        true,
        Network::Bitcoin,
        bitcoind_config(),
        0,
        Some(false),
        true,
    )
    .map(|_| ());
    dashboard_chrome(&SETTINGS_MENU, "Settings — bitcoind (stopped)", inner)
}

fn rpc_addr_form() -> &'static liana_ui::component::form::Value<String> {
    static F: OnceLock<liana_ui::component::form::Value<String>> = OnceLock::new();
    F.get_or_init(|| liana_ui::component::form::Value {
        value: "127.0.0.1:8332".to_string(),
        warning: None,
        valid: true,
    })
}

fn rpc_auth_values() -> &'static RpcAuthValues {
    static V: OnceLock<RpcAuthValues> = OnceLock::new();
    V.get_or_init(|| RpcAuthValues {
        cookie_path: liana_ui::component::form::Value {
            value: "/home/user/.bitcoin/.cookie".to_string(),
            warning: None,
            valid: true,
        },
        user: liana_ui::component::form::Value {
            value: "rpcuser".to_string(),
            warning: None,
            valid: true,
        },
        password: liana_ui::component::form::Value {
            value: "rpcpass".to_string(),
            warning: None,
            valid: true,
        },
    })
}

fn render_bitcoind_edit() -> Element<'static, DebugMessage> {
    let inner = v::bitcoind_edit(
        true,
        Network::Bitcoin,
        800_000,
        rpc_addr_form(),
        rpc_auth_values(),
        &RpcAuthType::UserPass,
        false,
    )
    .map(|_| ());
    dashboard_chrome(&SETTINGS_MENU, "Settings — bitcoind (edit form)", inner)
}

fn electrum_addr_form() -> &'static liana_ui::component::form::Value<String> {
    static F: OnceLock<liana_ui::component::form::Value<String>> = OnceLock::new();
    F.get_or_init(|| liana_ui::component::form::Value {
        value: "ssl://electrum.example:50002".to_string(),
        warning: None,
        valid: true,
    })
}

fn render_electrum_running() -> Element<'static, DebugMessage> {
    let inner = v::electrum(
        true,
        Network::Bitcoin,
        electrum_config(),
        800_000,
        Some(true),
        true,
    )
    .map(|_| ());
    dashboard_chrome(&SETTINGS_MENU, "Settings — electrum (running)", inner)
}

fn render_electrum_edit() -> Element<'static, DebugMessage> {
    let inner = v::electrum_edit(
        true,
        Network::Bitcoin,
        800_000,
        electrum_addr_form(),
        false,
        true,
    )
    .map(|_| ());
    dashboard_chrome(&SETTINGS_MENU, "Settings — electrum (edit form)", inner)
}

// ---- rescan ---------------------------------------------------------------

pub static ENTRY_RESCAN_IDLE: DebugPageEntry = DebugPageEntry {
    view: render_rescan_idle,
};
pub static ENTRY_RESCAN_SCANNING: DebugPageEntry = DebugPageEntry {
    view: render_rescan_scanning,
};
pub static ENTRY_RESCAN_SUCCESS: DebugPageEntry = DebugPageEntry {
    view: render_rescan_success,
};
pub static ENTRY_RESCAN_INVALID_DATE: DebugPageEntry = DebugPageEntry {
    view: render_rescan_invalid_date,
};

fn ymd_form(value: &str) -> liana_ui::component::form::Value<String> {
    liana_ui::component::form::Value {
        value: value.to_string(),
        warning: None,
        valid: true,
    }
}

fn render_rescan_idle() -> Element<'static, DebugMessage> {
    static FIELDS: OnceLock<(
        liana_ui::component::form::Value<String>,
        liana_ui::component::form::Value<String>,
        liana_ui::component::form::Value<String>,
    )> = OnceLock::new();
    let (y, m, d) = FIELDS.get_or_init(|| (ymd_form("2024"), ymd_form("01"), ymd_form("15")));
    let inner = v::rescan(y, m, d, None, false, false, true, false, false, false).map(|_| ());
    dashboard_chrome(&SETTINGS_MENU, "Settings — rescan (idle)", inner)
}

fn render_rescan_scanning() -> Element<'static, DebugMessage> {
    static FIELDS: OnceLock<(
        liana_ui::component::form::Value<String>,
        liana_ui::component::form::Value<String>,
        liana_ui::component::form::Value<String>,
    )> = OnceLock::new();
    let (y, m, d) = FIELDS.get_or_init(|| (ymd_form("2024"), ymd_form("01"), ymd_form("15")));
    let inner = v::rescan(y, m, d, Some(0.42), false, true, false, false, false, false).map(|_| ());
    dashboard_chrome(&SETTINGS_MENU, "Settings — rescan (scanning 42%)", inner)
}

fn render_rescan_success() -> Element<'static, DebugMessage> {
    static FIELDS: OnceLock<(
        liana_ui::component::form::Value<String>,
        liana_ui::component::form::Value<String>,
        liana_ui::component::form::Value<String>,
    )> = OnceLock::new();
    let (y, m, d) = FIELDS.get_or_init(|| (ymd_form("2024"), ymd_form("01"), ymd_form("15")));
    let inner = v::rescan(y, m, d, None, true, false, true, false, false, false).map(|_| ());
    dashboard_chrome(&SETTINGS_MENU, "Settings — rescan (success)", inner)
}

fn render_rescan_invalid_date() -> Element<'static, DebugMessage> {
    static FIELDS: OnceLock<(
        liana_ui::component::form::Value<String>,
        liana_ui::component::form::Value<String>,
        liana_ui::component::form::Value<String>,
    )> = OnceLock::new();
    let (y, m, d) = FIELDS.get_or_init(|| (ymd_form("2024"), ymd_form("13"), ymd_form("32")));
    let inner = v::rescan(y, m, d, None, false, false, true, true, false, false).map(|_| ());
    dashboard_chrome(&SETTINGS_MENU, "Settings — rescan (invalid date)", inner)
}

// ---- wallet_settings -----------------------------------------------------

const SAMPLE_DESCRIPTOR: &str = "wsh(or_d(pk([19608592/48'/1'/0'/2']tpubDEjf1AbrUjxnw8jg6Gi12CunPqnCobLP6Ktoy4Hd52pa65d6QRPg5CSkdFrqPDjJ8BAUuMEDVDRQVjtuWWksMqBeZCqyABFucN9ErQq8oVX/<0;1>/*),and_v(v:pkh([19608592/48'/1'/0'/2']tpubDEjf1AbrUjxnw8jg6Gi12CunPqnCobLP6Ktoy4Hd52pa65d6QRPg5CSkdFrqPDjJ8BAUuMEDVDRQVjtuWWksMqBeZCqyABFucN9ErQq8oVX/<2;3>/*),older(52596))))#x6u6lmej";

fn sample_descriptor() -> &'static LianaDescriptor {
    static D: OnceLock<LianaDescriptor> = OnceLock::new();
    D.get_or_init(|| {
        LianaDescriptor::from_str(SAMPLE_DESCRIPTOR).expect("sample descriptor parses")
    })
}

fn wallet_alias_form() -> &'static liana_ui::component::form::Value<String> {
    static F: OnceLock<liana_ui::component::form::Value<String>> = OnceLock::new();
    F.get_or_init(|| liana_ui::component::form::Value {
        value: "My wallet".to_string(),
        warning: None,
        valid: true,
    })
}

fn keys_aliases() -> &'static Vec<(Fingerprint, liana_ui::component::form::Value<String>)> {
    static V: OnceLock<Vec<(Fingerprint, liana_ui::component::form::Value<String>)>> =
        OnceLock::new();
    V.get_or_init(|| {
        vec![(
            Fingerprint::from([0x19, 0x60, 0x85, 0x92]),
            liana_ui::component::form::Value {
                value: "Vault key".to_string(),
                warning: None,
                valid: true,
            },
        )]
    })
}

fn empty_provider_keys() -> &'static HashMap<Fingerprint, crate::app::settings::ProviderKey> {
    static M: OnceLock<HashMap<Fingerprint, crate::app::settings::ProviderKey>> = OnceLock::new();
    M.get_or_init(HashMap::new)
}

pub static ENTRY_WALLET_SETTINGS: DebugPageEntry = DebugPageEntry {
    view: render_wallet_settings,
};
pub static ENTRY_WALLET_SETTINGS_PROCESSING: DebugPageEntry = DebugPageEntry {
    view: render_wallet_settings_processing,
};
pub static ENTRY_WALLET_SETTINGS_UPDATED: DebugPageEntry = DebugPageEntry {
    view: render_wallet_settings_updated,
};

fn render_wallet_settings() -> Element<'static, DebugMessage> {
    v::wallet_settings(
        static_cache(),
        None,
        sample_descriptor(),
        wallet_alias_form(),
        keys_aliases(),
        empty_provider_keys(),
        false,
        false,
    )
    .map(|_| ())
}

fn render_wallet_settings_processing() -> Element<'static, DebugMessage> {
    v::wallet_settings(
        static_cache(),
        None,
        sample_descriptor(),
        wallet_alias_form(),
        keys_aliases(),
        empty_provider_keys(),
        true,
        false,
    )
    .map(|_| ())
}

fn render_wallet_settings_updated() -> Element<'static, DebugMessage> {
    v::wallet_settings(
        static_cache(),
        None,
        sample_descriptor(),
        wallet_alias_form(),
        keys_aliases(),
        empty_provider_keys(),
        false,
        true,
    )
    .map(|_| ())
}

// ---- register_wallet_modal -----------------------------------------------

fn registered_set() -> &'static HashSet<Fingerprint> {
    static S: OnceLock<HashSet<Fingerprint>> = OnceLock::new();
    S.get_or_init(HashSet::new)
}

fn sample_hws_for_register() -> &'static Vec<HardwareWallet> {
    static V: OnceLock<Vec<HardwareWallet>> = OnceLock::new();
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
                None,
            ),
            hw_modals::locked(DeviceKind::Jade, Some("123-456")),
        ]
    })
}

pub static ENTRY_REGISTER_WALLET_MODAL: DebugPageEntry = DebugPageEntry {
    view: render_register_wallet_modal,
};

fn render_register_wallet_modal() -> Element<'static, DebugMessage> {
    let inner = v::register_wallet_modal(
        None,
        sample_hws_for_register(),
        false,
        None,
        registered_set(),
    )
    .map(|_| ());
    dashboard_with_modal(&SETTINGS_MENU, "Settings — register wallet modal", inner)
}

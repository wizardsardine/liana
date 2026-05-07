//! Wallet selection step.

use std::collections::BTreeSet;
use std::sync::OnceLock;

use liana_connect::ws_business::{Org, UserRole, Wallet, WalletStatus};
use liana_gui::debug::{installer_chrome, DebugMessage, DebugPageEntry};
use liana_ui::widget::Element;
use uuid::Uuid;

use crate::state::State;
use crate::views::wallet_select_view;

use super::{build_state, StateCell};

const WALLET_SELECT_PATH: &str = "business_installer::views::wallet_select::wallet_select_view";

pub static ENTRY_WALLET_SELECT_WITH_WALLETS: DebugPageEntry = DebugPageEntry {
    view: render_wallet_select_with_wallets,
};

fn wallet_select_with_wallets_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(build_state(|s| {
            s.app.global_user_role = Some(UserRole::WizardSardineAdmin);
            // Production hides Finalized for WS Admins via the
            // `hide_finalized` checkbox; flip it off here so the Active
            // pill is visible alongside the others.
            s.views.wallet_select.hide_finalized = false;
            let org_id = Uuid::from_u128(0x2000);
            // One wallet per realistic `WalletStatus` (Created is a
            // transient backend-only state never surfaced to this view in
            // production, so we skip it).
            let entries: &[(&str, WalletStatus)] = &[
                ("Drafted wallet", WalletStatus::Drafted),
                ("Locked wallet", WalletStatus::Locked),
                ("Validated wallet", WalletStatus::Validated),
                ("Registration wallet", WalletStatus::Registration),
                ("Active wallet", WalletStatus::Finalized),
            ];
            let mut wallet_ids = BTreeSet::new();
            {
                let mut wallets = s.backend.wallets.lock().expect("poisoned");
                for (i, (alias, status)) in entries.iter().enumerate() {
                    let id = Uuid::from_u128(0x3000 + i as u128);
                    wallet_ids.insert(id);
                    wallets.insert(
                        id,
                        Wallet {
                            alias: (*alias).to_string(),
                            org: org_id,
                            owner: Uuid::nil(),
                            id,
                            status: *status,
                            template: None,
                            last_edited: None,
                            last_editor: None,
                            descriptor: None,
                            devices: None,
                        },
                    );
                }
            }
            {
                let mut orgs = s.backend.orgs.lock().expect("poisoned");
                orgs.insert(
                    org_id,
                    Org {
                        name: "Acme Vault".to_string(),
                        id: org_id,
                        wallets: wallet_ids,
                        users: BTreeSet::new(),
                        owners: Vec::new(),
                        last_edited: None,
                        last_editor: None,
                    },
                );
            }
            s.app.selected_org = Some(org_id);
        }))
    })
    .0
}

fn render_wallet_select_with_wallets() -> Element<'static, DebugMessage> {
    let body = wallet_select_view(wallet_select_with_wallets_state()).map(|_| ());
    installer_chrome(
        "Business installer — wallet select (with wallets)",
        WALLET_SELECT_PATH,
        body,
    )
}

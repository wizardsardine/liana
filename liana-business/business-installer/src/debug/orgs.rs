//! Organization selection step.

use std::collections::BTreeSet;
use std::sync::OnceLock;

use liana_connect::ws_business::{Org, UserRole, Wallet, WalletStatus};
use liana_gui::debug::{installer_chrome, DebugMessage, DebugPageEntry};
use liana_ui::widget::Element;
use uuid::Uuid;

use crate::state::State;
use crate::views::org_select_view;

use super::{build_state, StateCell};

const ORG_SELECT_PATH: &str = "business_installer::views::org_select::org_select_view";

pub static ENTRY_ORG_SELECT_WITH_ORGS: DebugPageEntry = DebugPageEntry {
    view: render_org_select_with_orgs,
};

fn org_select_with_orgs_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(build_state(|s| {
            s.app.global_user_role = Some(UserRole::WizardSardineAdmin);
            // org_select_view hides any org whose accessible-wallet count
            // is zero, so each mock org needs at least one wallet
            // registered in the backend.
            let names = ["Acme Vault", "Treasury Co", "Cold Storage Inc."];
            let mut wallets_guard = s.backend.wallets.lock().expect("poisoned");
            let mut orgs_guard = s.backend.orgs.lock().expect("poisoned");
            for (i, name) in names.iter().enumerate() {
                let org_id = Uuid::from_u128(0x1000 + i as u128);
                let wallet_id = Uuid::from_u128(0x6000 + i as u128);
                wallets_guard.insert(
                    wallet_id,
                    Wallet {
                        alias: format!("{name} treasury"),
                        org: org_id,
                        owner: Uuid::nil(),
                        id: wallet_id,
                        // Not `Finalized`: WS Admins default to
                        // `hide_finalized = true`, which would zero out
                        // the org's accessible-wallet count and cause
                        // `org_select_view` to skip the row entirely.
                        status: WalletStatus::Drafted,
                        template: None,
                        last_edited: None,
                        last_editor: None,
                        descriptor: None,
                        devices: None,
                    },
                );
                let mut org_wallets = BTreeSet::new();
                org_wallets.insert(wallet_id);
                orgs_guard.insert(
                    org_id,
                    Org {
                        name: (*name).to_string(),
                        id: org_id,
                        wallets: org_wallets,
                        users: BTreeSet::new(),
                        owners: Vec::new(),
                        last_edited: None,
                        last_editor: None,
                    },
                );
            }
        }))
    })
    .0
}

fn render_org_select_with_orgs() -> Element<'static, DebugMessage> {
    let body = org_select_view(org_select_with_orgs_state()).map(|_| ());
    installer_chrome(
        "Business installer — org select (with orgs)",
        ORG_SELECT_PATH,
        body,
    )
}

use crate::state::{message::Msg, State, View};
use liana_connect::ws_business::{UserRole, WalletStatus};
use liana_ui::{
    component::tab::{self, Dot},
    widget::*,
};

const SPENDING_POLICY_TAB_LABEL: &str = "Spending policy";
const KEYS_SIGNERS_TAB_LABEL: &str = "Keys & Signers";

fn template_edit_disabled(
    current_user_role: Option<UserRole>,
    wallet_status: Option<WalletStatus>,
    keys_ready: bool,
) -> bool {
    matches!(current_user_role, Some(UserRole::WalletManager))
        && matches!(
            wallet_status,
            Some(WalletStatus::Created | WalletStatus::Drafted)
        )
        && !keys_ready
}

fn wallet_edit_view(
    current_view: View,
    current_user_role: Option<UserRole>,
    wallet_status: Option<WalletStatus>,
    keys_ready: bool,
) -> View {
    if current_view == View::TemplateEdit
        && template_edit_disabled(current_user_role, wallet_status, keys_ready)
    {
        View::Keys
    } else {
        current_view
    }
}

fn wallet_edit_tab_items(
    keys_ready: bool,
    template_edit_disabled: bool,
) -> Vec<(View, &'static str, Option<Dot>)> {
    let dot = if keys_ready { Dot::Ready } else { Dot::Pending };
    let keys = (View::Keys, KEYS_SIGNERS_TAB_LABEL, Some(dot));
    if template_edit_disabled {
        vec![keys]
    } else {
        vec![(View::TemplateEdit, SPENDING_POLICY_TAB_LABEL, None), keys]
    }
}

pub(crate) fn wallet_edit_route(state: &State) -> View {
    let wallet_status = state.selected_wallet().map(|wallet| wallet.status);

    wallet_edit_view(
        state.current_view,
        state.app.current_user_role,
        wallet_status,
        state.app.keys_ready,
    )
}

pub(crate) fn wallet_edit_tab_header(state: &State) -> Element<'_, Msg> {
    let wallet_status = state.selected_wallet().map(|wallet| wallet.status);
    let gated = template_edit_disabled(
        state.app.current_user_role,
        wallet_status,
        state.app.keys_ready,
    );
    let active = wallet_edit_view(
        state.current_view,
        state.app.current_user_role,
        wallet_status,
        state.app.keys_ready,
    );
    let items = wallet_edit_tab_items(state.app.keys_ready, gated);

    Container::new(tab::tab_header(&items, &active, |view| {
        if *view == View::TemplateEdit {
            Msg::NavigateToHome
        } else {
            Msg::NavigateToKeys
        }
    }))
    .into()
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn wallet_edit_tabs_show_for_ws_admin() {
        let items = wallet_edit_tab_items(
            false,
            template_edit_disabled(
                Some(UserRole::WizardSardineAdmin),
                Some(WalletStatus::Drafted),
                false,
            ),
        );

        assert_eq!(
            items,
            [
                (View::TemplateEdit, SPENDING_POLICY_TAB_LABEL, None),
                (View::Keys, KEYS_SIGNERS_TAB_LABEL, Some(Dot::Pending)),
            ]
        );
    }

    #[test]
    fn wallet_edit_tabs_show_for_non_draft_manager() {
        let items = wallet_edit_tab_items(
            false,
            template_edit_disabled(
                Some(UserRole::WalletManager),
                Some(WalletStatus::Locked),
                false,
            ),
        );

        assert_eq!(items.len(), 2);
        assert_eq!(items[0].1, SPENDING_POLICY_TAB_LABEL);
        assert_eq!(items[1].1, KEYS_SIGNERS_TAB_LABEL);
    }

    #[test]
    fn draft_manager_without_ready_keys_only_sees_keys_tab() {
        let gated = template_edit_disabled(
            Some(UserRole::WalletManager),
            Some(WalletStatus::Drafted),
            false,
        );
        let items = wallet_edit_tab_items(false, gated);

        assert!(gated);
        assert_eq!(
            items,
            [(View::Keys, KEYS_SIGNERS_TAB_LABEL, Some(Dot::Pending))]
        );
        assert_eq!(
            wallet_edit_view(
                View::TemplateEdit,
                Some(UserRole::WalletManager),
                Some(WalletStatus::Drafted),
                false,
            ),
            View::Keys
        );
    }

    #[test]
    fn created_manager_without_ready_keys_only_sees_keys_tab() {
        let gated = template_edit_disabled(
            Some(UserRole::WalletManager),
            Some(WalletStatus::Created),
            false,
        );
        let items = wallet_edit_tab_items(false, gated);

        assert!(gated);
        assert_eq!(
            items,
            [(View::Keys, KEYS_SIGNERS_TAB_LABEL, Some(Dot::Pending))]
        );
        assert_eq!(
            wallet_edit_view(
                View::TemplateEdit,
                Some(UserRole::WalletManager),
                Some(WalletStatus::Created),
                false,
            ),
            View::Keys
        );
    }

    #[test]
    fn ready_keys_restore_policy_tab_without_switching_views() {
        let gated = template_edit_disabled(
            Some(UserRole::WalletManager),
            Some(WalletStatus::Drafted),
            true,
        );
        let items = wallet_edit_tab_items(true, gated);

        assert!(!gated);
        assert_eq!(items.len(), 2);
        assert_eq!(
            wallet_edit_view(
                View::Keys,
                Some(UserRole::WalletManager),
                Some(WalletStatus::Drafted),
                true,
            ),
            View::Keys
        );
    }
}

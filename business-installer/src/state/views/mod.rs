pub mod keys;
pub mod login;
pub mod modals;
pub mod org_select;
pub mod path;
pub mod template_builder;
pub mod wallet_select;
pub mod xpub;

pub use keys::{EditKeyModalState, KeysViewState};
pub use login::{Login, LoginState};
pub use modals::ModalsState;
pub use org_select::OrgSelectState;
pub use path::{EditPathModalState, PathsViewState};
pub use wallet_select::WalletSelectState;

/// View-specific states
#[derive(Debug, Clone)]
pub struct ViewsState {
    pub modals: ModalsState,
    pub keys: KeysViewState,
    pub paths: PathsViewState,
    pub login: Login,
    pub org_select: OrgSelectState,
    pub wallet_select: WalletSelectState,
}

impl ViewsState {
    pub fn new() -> Self {
        Self {
            modals: ModalsState::default(),
            keys: KeysViewState::default(),
            paths: PathsViewState::default(),
            login: Login::default(),
            org_select: OrgSelectState::default(),
            wallet_select: WalletSelectState::default(),
        }
    }
}

impl Default for ViewsState {
    fn default() -> Self {
        Self::new()
    }
}

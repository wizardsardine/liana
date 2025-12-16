pub mod home;
pub mod keys;
pub mod login;
pub mod modals;
pub mod org_select;
pub mod path;
pub mod wallet_select;

pub use keys::{EditKeyModalState, KeysViewState};
pub use login::{Login, LoginState};
pub use modals::ModalsState;
pub use path::{EditPathModalState, PathsViewState};

/// View-specific states
#[derive(Debug, Clone)]
pub struct ViewsState {
    pub modals: ModalsState,
    pub keys: KeysViewState,
    pub paths: PathsViewState,
    pub login: Login,
}

impl ViewsState {
    pub fn new() -> Self {
        Self {
            modals: ModalsState::default(),
            keys: KeysViewState::default(),
            paths: PathsViewState::default(),
            login: Login::default(),
        }
    }
}

impl Default for ViewsState {
    fn default() -> Self {
        Self::new()
    }
}

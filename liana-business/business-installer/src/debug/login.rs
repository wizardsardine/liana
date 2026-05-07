//! Loading + login pages (account select / email / code).

use std::sync::OnceLock;

use liana_gui::debug::{installer_chrome, DebugMessage, DebugPageEntry};
use liana_ui::widget::Element;

use crate::state::{
    views::login::{CachedAccount, Login, LoginState},
    State, View,
};
use crate::views;

use super::{build_state, stub_tokens, StateCell};

const LOADING_PATH: &str = "business_installer::views::loading::loading_view";
const ACCOUNT_SELECT_PATH: &str =
    "business_installer::views::login::account_select::account_select_view";
const EMAIL_PATH: &str = "business_installer::views::login::email::login_email_view";
const CODE_PATH: &str = "business_installer::views::login::code::login_code_view";

pub static ENTRY_LOADING_OK: DebugPageEntry = DebugPageEntry {
    view: render_loading_ok,
};
pub static ENTRY_LOADING_ERROR: DebugPageEntry = DebugPageEntry {
    view: render_loading_error,
};
pub static ENTRY_ACCOUNT_SELECT: DebugPageEntry = DebugPageEntry {
    view: render_account_select,
};
pub static ENTRY_ACCOUNT_SELECT_MANY: DebugPageEntry = DebugPageEntry {
    view: render_account_select_many,
};
pub static ENTRY_ACCOUNT_SELECT_PROCESSING: DebugPageEntry = DebugPageEntry {
    view: render_account_select_processing,
};
pub static ENTRY_EMAIL_EMPTY: DebugPageEntry = DebugPageEntry {
    view: render_email_empty,
};
pub static ENTRY_EMAIL: DebugPageEntry = DebugPageEntry { view: render_email };
pub static ENTRY_EMAIL_INVALID: DebugPageEntry = DebugPageEntry {
    view: render_email_invalid,
};
pub static ENTRY_CODE_EMPTY: DebugPageEntry = DebugPageEntry {
    view: render_code_empty,
};
pub static ENTRY_CODE: DebugPageEntry = DebugPageEntry { view: render_code };
pub static ENTRY_CODE_INVALID: DebugPageEntry = DebugPageEntry {
    view: render_code_invalid,
};

// ---- loading -------------------------------------------------------------

fn render_loading_ok() -> Element<'static, DebugMessage> {
    let body = views::loading_view(false).map(|_| ());
    installer_chrome("Business installer — loading", LOADING_PATH, body)
}

fn render_loading_error() -> Element<'static, DebugMessage> {
    let body = views::loading_view(true).map(|_| ());
    installer_chrome("Business installer — loading (error)", LOADING_PATH, body)
}

// ---- account select ------------------------------------------------------
fn account_select_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(build_state(|s| {
            s.current_view = View::Login;
            let accounts = vec![
                CachedAccount {
                    email: "alice@example.com".to_string(),
                    tokens: stub_tokens(),
                },
                CachedAccount {
                    email: "bob@example.com".to_string(),
                    tokens: stub_tokens(),
                },
            ];
            s.views.login = Login::with_cached_accounts(accounts);
        }))
    })
    .0
}

fn render_account_select() -> Element<'static, DebugMessage> {
    let body = views::login_view(account_select_state()).map(|_| ());
    installer_chrome(
        "Business installer — login (account select)",
        ACCOUNT_SELECT_PATH,
        body,
    )
}

fn account_select_many_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(build_state(|s| {
            s.current_view = View::Login;
            // Eight accounts — overflows the viewport so the inner scroll
            // is visible.
            let accounts = (0..8)
                .map(|i| CachedAccount {
                    email: format!("user{i}@example.com"),
                    tokens: stub_tokens(),
                })
                .collect();
            s.views.login = Login::with_cached_accounts(accounts);
        }))
    })
    .0
}

fn render_account_select_many() -> Element<'static, DebugMessage> {
    let body = views::login_view(account_select_many_state()).map(|_| ());
    installer_chrome(
        "Business installer — login (account select, many cached)",
        ACCOUNT_SELECT_PATH,
        body,
    )
}

fn account_select_processing_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(build_state(|s| {
            s.current_view = View::Login;
            // Two accounts: only the selected one shows "Connecting…".
            let accounts = vec![
                CachedAccount {
                    email: "alice@example.com".to_string(),
                    tokens: stub_tokens(),
                },
                CachedAccount {
                    email: "bob@example.com".to_string(),
                    tokens: stub_tokens(),
                },
            ];
            s.views.login = Login::with_cached_accounts(accounts);
            s.views.login.account_select.processing = true;
            s.views.login.account_select.selected_email = Some("alice@example.com".to_string());
        }))
    })
    .0
}

fn render_account_select_processing() -> Element<'static, DebugMessage> {
    let body = views::login_view(account_select_processing_state()).map(|_| ());
    installer_chrome(
        "Business installer — login (account select, connecting)",
        ACCOUNT_SELECT_PATH,
        body,
    )
}

// ---- email --------------------------------------------------------------

fn email_empty_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(build_state(|s| {
            s.current_view = View::Login;
            s.views.login.current = LoginState::EmailEntry;
        }))
    })
    .0
}

fn render_email_empty() -> Element<'static, DebugMessage> {
    let body = views::login_view(email_empty_state()).map(|_| ());
    installer_chrome(
        "Business installer — login (email entry, empty)",
        EMAIL_PATH,
        body,
    )
}

fn email_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(build_state(|s| {
            s.current_view = View::Login;
            s.views.login.current = LoginState::EmailEntry;
            s.views
                .login
                .on_update_email("alice@example.com".to_string());
        }))
    })
    .0
}

fn render_email() -> Element<'static, DebugMessage> {
    let body = views::login_view(email_state()).map(|_| ());
    installer_chrome("Business installer — login (email entry)", EMAIL_PATH, body)
}

fn email_invalid_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(build_state(|s| {
            s.current_view = View::Login;
            s.views.login.current = LoginState::EmailEntry;
            s.views.login.on_update_email("not-an-email".to_string());
        }))
    })
    .0
}

fn render_email_invalid() -> Element<'static, DebugMessage> {
    let body = views::login_view(email_invalid_state()).map(|_| ());
    installer_chrome(
        "Business installer — login (email entry, invalid)",
        EMAIL_PATH,
        body,
    )
}

// ---- code ---------------------------------------------------------------

fn code_empty_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(build_state(|s| {
            s.current_view = View::Login;
            s.views.login.current = LoginState::CodeEntry;
            s.views
                .login
                .on_update_email("alice@example.com".to_string());
        }))
    })
    .0
}

fn render_code_empty() -> Element<'static, DebugMessage> {
    let body = views::login_view(code_empty_state()).map(|_| ());
    installer_chrome(
        "Business installer — login (code entry, empty)",
        CODE_PATH,
        body,
    )
}

fn code_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(build_state(|s| {
            s.current_view = View::Login;
            s.views.login.current = LoginState::CodeEntry;
            s.views
                .login
                .on_update_email("alice@example.com".to_string());
            s.views.login.on_update_code("123456".to_string());
        }))
    })
    .0
}

fn render_code() -> Element<'static, DebugMessage> {
    let body = views::login_view(code_state()).map(|_| ());
    installer_chrome("Business installer — login (code entry)", CODE_PATH, body)
}

fn code_invalid_state() -> &'static State {
    static S: OnceLock<StateCell<State>> = OnceLock::new();
    &S.get_or_init(|| {
        StateCell(build_state(|s| {
            s.current_view = View::Login;
            s.views.login.current = LoginState::CodeEntry;
            s.views
                .login
                .on_update_email("alice@example.com".to_string());
            s.views.login.on_update_code("12abcd".to_string());
        }))
    })
    .0
}

fn render_code_invalid() -> Element<'static, DebugMessage> {
    let body = views::login_view(code_invalid_state()).map(|_| ());
    installer_chrome(
        "Business installer — login (code entry, invalid)",
        CODE_PATH,
        body,
    )
}

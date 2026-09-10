pub mod keys;
pub mod loading;
pub mod login;
pub mod modals;
pub mod org_select;
pub mod paths;
pub mod registration;
pub mod wallet_edit;
pub mod wallet_select;
pub mod xpub;

pub use keys::keys_view;
use liana_connect::ws_business::{self, UserRole};
pub use liana_ui::component::installer::{intro_description, intro_prompt, screen_intro};
use liana_ui::component::{installer as installer_layout, text::capitalize_first};
pub use loading::loading_view;
pub use login::login_view;
use miniscript::bitcoin::Network;
pub use org_select::org_select_view;
pub use paths::template_builder_view;
pub use registration::registration_view;
pub use wallet_select::wallet_select_view;
pub use xpub::xpub_view;

use crate::{
    backend::Backend,
    state::{message::Msg, State},
};
use iced::{
    widget::{column, container, Space},
    Alignment, Length,
};
use liana_i18n::t;
use liana_ui::{
    component::{
        button::{self, EntryWidth},
        form, list,
        text::{self, short_email, truncate},
        tooltip,
    },
    spacing::VSpacing,
    theme,
    widget::*,
    Variant,
};
use uuid::Uuid;

pub const INSTALLER_STEPS: usize = 7;
pub const MENU_ENTRY_HEIGHT: u32 = 100;
const CONTENT_WIDTH: f32 = button::STANDARD_ENTRY_WIDTH;

/// Build the collapsed line and the hover detail for a last-edit info.
/// Visible: "Edited <relative time>". Hover: "Edited by <editor> on <absolute time>".
fn format_last_edit_info_strings(
    last_edited: Option<u64>,
    last_editor: Option<Uuid>,
    state: &State,
    current_user_email_lower: &str,
) -> Option<(String, String)> {
    let timestamp = last_edited?;
    let visible = t!(
        "business-edited-relative",
        editor = "",
        time = state.app.format_relative_time(timestamp)
    );

    let editor = last_editor.and_then(|editor_id| {
        state.backend.get_user(editor_id).map(|user| {
            if user.email.to_lowercase() == current_user_email_lower {
                t!("business-common-you")
            } else if user.role == UserRole::WizardSardineAdmin {
                let name = admin_name_from_email(&user.email).unwrap_or_default();
                t!("business-admin-name", name = name)
            } else {
                user.email.clone()
            }
        })
    });
    let absolute = state.app.format_absolute_time(timestamp);
    let hover = match (editor, absolute.is_empty()) {
        (Some(editor), false) => {
            t!("business-edited-by-on", editor = editor, date = absolute)
        }
        (Some(editor), true) => t!("business-edited-by-user", editor = editor),
        (None, false) => t!("business-edited-on", date = absolute),
        (None, true) => t!("business-edited"),
    };

    Some((visible, hover))
}

pub fn format_last_edit_info<'a, M>(
    last_edited: Option<u64>,
    last_editor: Option<Uuid>,
    state: &State,
    current_user_email_lower: &str,
) -> Option<Element<'a, M>>
where
    M: 'a + Clone,
{
    format_last_edit_info_strings(last_edited, last_editor, state, current_user_email_lower).map(
        |(visible, hover)| {
            tooltip::tooltip_custom(
                text::new::caption(hover).style(theme::text::secondary),
                text::new::caption(visible).style(theme::text::secondary),
                iced::widget::tooltip::Position::Bottom,
            )
            .into()
        },
    )
}

fn admin_name_from_email(mail: &str) -> Option<String> {
    let mail = mail.split_once('@').map(|(a, _)| a)?;
    let split_plus = mail.split_once('+').map(|(a, _)| a);
    let n = split_plus
        .map(|p| if p.len() < mail.len() { p } else { mail })
        .unwrap_or(mail);
    Some(format!("({})", capitalize_first(n)))
}

fn layout_inner<'a>(
    progress: (usize, usize),
    network: Network,
    email: Option<&'a str>,
    is_ws_admin: bool,
    breadcrumb: &[String],
    content: installer_layout::LayoutContent<'a, Msg>,
    previous_message: Option<Msg>,
) -> Element<'a, Msg> {
    let previous_message = previous_message.or_else(|| email.map(|_| Msg::Disconnect));
    installer_layout::layout_inner(
        installer_layout::LayoutConfig {
            variant: Variant::LianaBusiness,
            network,
            email: email.map(String::from),
            is_ws_admin,
            nav_bar: installer_layout::NavBar::Steps {
                progress,
                breadcrumb: breadcrumb.to_vec(),
                previous_message,
            },
            content_width: CONTENT_WIDTH,
        },
        content,
    )
}

pub fn layout<'a>(
    progress: (usize, usize),
    network: Network,
    email: Option<&'a str>,
    breadcrumb: &[String],
    content: impl Into<Element<'a, Msg>>,
    previous_message: Option<Msg>,
) -> Element<'a, Msg> {
    layout_inner(
        progress,
        network,
        email,
        false,
        breadcrumb,
        installer_layout::LayoutContent::Scrollable(content.into()),
        previous_message,
    )
}

/// Layout variant with fixed header content and a scrollable list section.
/// The header_content stays fixed at top, only the list_content scrolls.
/// Optional pinned_content and footer_content can be placed below the scrollable area.
#[allow(clippy::too_many_arguments)]
pub fn layout_with_scrollable_list<'a>(
    progress: (usize, usize),
    network: Network,
    email: Option<&'a str>,
    is_ws_admin: bool,
    breadcrumb: &[String],
    header_content: Option<Element<'a, Msg>>,
    list_content: impl Into<Element<'a, Msg>>,
    pinned_content: Option<Element<'a, Msg>>,
    footer_content: Option<Element<'a, Msg>>,
    previous_message: Option<Msg>,
) -> Element<'a, Msg> {
    layout_inner(
        progress,
        network,
        email,
        is_ws_admin,
        breadcrumb,
        installer_layout::LayoutContent::ScrollableList {
            header: header_content,
            list: list_content.into(),
            pinned: pinned_content,
            footer: footer_content,
        },
        previous_message,
    )
}

// NOTE: content MUST have width and height to Length::Fill
pub fn menu_entry(content: Row<'_, Msg>, message: Option<Msg>) -> Container<'_, Msg> {
    let content = content.width(Length::Fill).height(Length::Fill);
    let card = button::list_entry(content, None, EntryWidth::Standard, message);
    container(card)
        .width(button::STANDARD_ENTRY_WIDTH)
        .height(MENU_ENTRY_HEIGHT)
}

pub fn menu_key_entry(
    key: &ws_business::Key,
    signer: String,
    kind_pill: Element<'static, Msg>,
    trailing: Element<'static, Msg>,
    msg: Option<Msg>,
    on_delete: Option<Msg>,
) -> Element<'static, Msg> {
    let alias = truncate(&key.alias, 25);

    list::entry_key(
        entry_key_kind(&key.key_type),
        alias,
        kind_pill,
        short_email(&signer, 40),
        Some(trailing),
        msg,
        on_delete,
    )
}

pub(crate) fn key_kind_label(key_type: &ws_business::KeyType) -> String {
    match key_type {
        ws_business::KeyType::Internal => t!("pill-key-internal"),
        ws_business::KeyType::External => t!("pill-key-external"),
        ws_business::KeyType::Cosigner => t!("pill-key-cosigner"),
        ws_business::KeyType::SafetyNet => t!("pill-key-safety-net"),
    }
}

pub(crate) fn entry_key_kind(key_type: &ws_business::KeyType) -> list::EntryKeyKind {
    match key_type {
        ws_business::KeyType::Internal => list::EntryKeyKind::Internal,
        ws_business::KeyType::External => list::EntryKeyKind::External,
        ws_business::KeyType::Cosigner | ws_business::KeyType::SafetyNet => {
            list::EntryKeyKind::SafetyNet
        }
    }
}

pub(crate) const SEARCH_ENTRY_THRESHOLD: usize = 5;

/// Optional centered search bar inside a select list view.
pub struct SelectSearch<'a> {
    pub placeholder: String,
    pub value: &'a str,
    pub on_change: fn(String) -> Msg,
}

/// Standard "pick one from a list" page used by org_select and wallet_select.
pub struct SelectListView<'a> {
    pub progress: (usize, usize),
    pub network: Network,
    pub email: &'a str,
    pub is_ws_admin: bool,
    pub breadcrumb: Vec<String>,
    pub title: String,
    pub search: Option<SelectSearch<'a>>,
    pub list: Column<'static, Msg>,
    pub previous_message: Option<Msg>,
}

pub fn select_list_view(cfg: SelectListView<'_>) -> Element<'_, Msg> {
    let mut header = column![screen_intro(cfg.title, None, false)]
        .spacing(VSpacing::M)
        .align_x(Alignment::Center)
        .padding([0, 20]);

    if let Some(search) = cfg.search {
        let value = form::Value {
            value: search.value.to_string(),
            warning: None,
            valid: true,
        };
        let search_form = form::Form::new_trimmed(&search.placeholder, &value, search.on_change)
            .size(16)
            .padding(10);
        let search_container = Container::new(search_form).align_x(Alignment::Center);
        header = header.push(search_container);
    }

    let list = cfg.list.push(Space::with_height(50));

    layout_with_scrollable_list(
        cfg.progress,
        cfg.network,
        Some(cfg.email),
        cfg.is_ws_admin,
        &cfg.breadcrumb,
        Some(header.into()),
        list,
        None,
        None,
        cfg.previous_message,
    )
}

#[cfg(test)]
mod test {
    use crate::views::admin_name_from_email;

    #[test]
    fn email_admin() {
        let mut mail = "manu+admin@wizardsardine.com";
        assert_eq!(admin_name_from_email(mail).unwrap(), "(Manu)");
        mail = "bob@wiz+sardine.com";
        assert_eq!(admin_name_from_email(mail).unwrap(), "(Bob)");
        mail = "kevin@WizardSardine.com";
        assert_eq!(admin_name_from_email(mail).unwrap(), "(Kevin)");
    }
}

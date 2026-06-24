pub mod keys;
pub mod loading;
pub mod login;
pub mod modals;
pub mod org_select;
pub mod paths;
pub mod registration;
pub mod template_builder;
pub mod wallet_select;
pub mod xpub;

pub use keys::keys_view;
use liana_connect::ws_business::{self, UserRole};
use liana_ui::component::{button::btn_breadcrumb_previous, text::capitalize_first};
pub use loading::loading_view;
pub use login::login_view;
pub use org_select::org_select_view;
pub use registration::registration_view;
pub use template_builder::template_builder_view;
pub use wallet_select::wallet_select_view;
pub use xpub::xpub_view;

use crate::{backend::Backend, state::message::Msg, state::State};
use iced::{
    widget::{container, Space},
    Alignment, Length,
};
use liana_ui::{
    component::{
        button::{self, icon_btn, EntryWidth},
        form, list, scrollable,
        text::{self, short_email, truncate},
    },
    icon, theme,
    widget::*,
};
use uuid::Uuid;

pub const INSTALLER_STEPS: usize = 5;
pub const MENU_ENTRY_WIDTH: u32 = 600;
pub const MENU_ENTRY_HEIGHT: u32 = 100;
const EMAIL_ROW_HEIGHT: u32 = 56;

pub fn format_last_edit_info(
    last_edited: Option<u64>,
    last_editor: Option<Uuid>,
    state: &State,
    current_user_email_lower: &str,
) -> Option<String> {
    let timestamp = last_edited?;
    let editor_str = last_editor
        .and_then(|editor_id| {
            state.backend.get_user(editor_id).map(|user| {
                if user.email.to_lowercase() == current_user_email_lower {
                    "You".to_string()
                } else if user.role == UserRole::WizardSardineAdmin {
                    let name = admin_name_from_email(&user.email).unwrap_or_default();
                    format!("Admin{name}")
                } else {
                    user.email.clone()
                }
            })
        })
        .map(|name| format!(" by {name}"))
        .unwrap_or_default();
    let relative_time = state.app.format_relative_time(timestamp);
    Some(format!("Edited{editor_str} {relative_time}"))
}

fn admin_name_from_email(mail: &str) -> Option<String> {
    let mail = mail.split_once('@').map(|(a, _)| a)?;
    let split_plus = mail.split_once('+').map(|(a, _)| a);
    let n = split_plus
        .map(|p| if p.len() < mail.len() { p } else { mail })
        .unwrap_or(mail);
    Some(format!("({})", capitalize_first(n)))
}

const EMAIL_HEADER_SPACER: u32 = 30;

/// Create a breadcrumb element from path segments.
/// Renders as "Segment1 > Segment2 > Segment3" with styled separators.
/// All segments have the same font size (h3), with `>` separators in secondary style.
fn breadcrumb_header<'a>(segments: &[String]) -> Element<'a, Msg> {
    let mut row = Row::new().spacing(8).align_y(Alignment::Center);

    for (i, segment) in segments.iter().enumerate() {
        if i > 0 {
            // Add separator
            row = row.push(text::h3(">").style(theme::text::secondary));
        }
        row = row.push(text::h3(segment));
    }

    row.into()
}

enum LayoutContent<'a> {
    Scrollable(Element<'a, Msg>),
    ScrollableList {
        header: Element<'a, Msg>,
        list: Element<'a, Msg>,
        footer: Option<Element<'a, Msg>>,
    },
}

fn layout_inner<'a>(
    progress: (usize, usize),
    email: Option<&'a str>,
    is_ws_admin: bool,
    breadcrumb: &[String],
    content: LayoutContent<'a>,
    padding_left: bool,
    previous_message: Option<Msg>,
) -> Element<'a, Msg> {
    let has_left_button = previous_message.is_some() || email.is_some();
    let msg = if let Some(msg) = previous_message {
        Some(msg)
    } else if email.is_some() {
        Some(Msg::Disconnect)
    } else {
        None
    };
    let left_button = btn_breadcrumb_previous(msg);

    // Build the top-right row with optional role badge and email
    let mut email_row = Row::new()
        .push(Space::with_width(Length::Fill))
        .spacing(10)
        .align_y(Alignment::Center);

    if is_ws_admin {
        email_row = email_row.push(liana_ui::component::pill::ws_admin());
    }

    if let Some(e) = email {
        email_row = email_row
            .push(Container::new(text::p1_medium(e).style(theme::text::accent)).padding(20));
    } else {
        email_row = email_row.push(Space::with_height(EMAIL_ROW_HEIGHT));
    }

    let header = Row::new()
        .height(EMAIL_ROW_HEIGHT)
        .align_y(Alignment::Center)
        .push(if has_left_button {
            Container::new(left_button)
                .center_x(Length::FillPortion(2))
                .into()
        } else {
            Element::from(Space::with_width(Length::FillPortion(2)))
        })
        .push(Container::new(breadcrumb_header(breadcrumb)).width(Length::FillPortion(8)))
        .push_maybe(if progress.1 > 0 {
            Some(
                Container::new(text::text(format!("{} | {}", progress.0, progress.1)))
                    .center_x(Length::FillPortion(2)),
            )
        } else {
            None
        });

    let fill_portion = if padding_left { 8 } else { 10 };
    let right_spacer = || -> Option<Space> {
        if padding_left {
            Some(Space::with_width(Length::FillPortion(2)))
        } else {
            None
        }
    };

    match content {
        LayoutContent::Scrollable(inner) => {
            let content_row = Row::new()
                .push(Space::with_width(Length::FillPortion(2)))
                .push(
                    Container::new(
                        Column::new()
                            .push(Space::with_height(Length::Fixed(100.0)))
                            .push(inner),
                    )
                    .width(Length::FillPortion(fill_portion)),
                )
                .push_maybe(right_spacer());

            Container::new(scrollable::vertical(
                Column::new()
                    .width(Length::Fill)
                    .push(email_row)
                    .push(Space::with_height(EMAIL_HEADER_SPACER))
                    .push(header)
                    .push(content_row),
            ))
            .center_x(Length::Fill)
            .height(Length::Fill)
            .width(Length::Fill)
            .style(theme::container::background)
            .into()
        }
        LayoutContent::ScrollableList {
            header: header_content,
            list,
            footer,
        } => {
            let header_area = Row::new()
                .push(Space::with_width(Length::FillPortion(2)))
                .push(Container::new(header_content).width(Length::FillPortion(fill_portion)))
                .push_maybe(right_spacer());

            let list_area = Row::new()
                .push(Space::with_width(Length::FillPortion(2)))
                .push(
                    Container::new(scrollable::vertical(list).height(Length::Fill))
                        .width(Length::FillPortion(fill_portion))
                        .align_x(Alignment::Center),
                )
                .push_maybe(right_spacer())
                .height(Length::Fill);

            let footer_area: Option<Row<'a, Msg>> = footer.map(|f| {
                Row::new()
                    .push(Space::with_width(Length::FillPortion(2)))
                    .push(Container::new(f).width(Length::FillPortion(fill_portion)))
                    .push_maybe(right_spacer())
            });

            Container::new(
                Column::new()
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .push(email_row)
                    .push(Space::with_height(EMAIL_HEADER_SPACER))
                    .push(header)
                    .push(header_area)
                    .push(list_area)
                    .push_maybe(footer_area),
            )
            .center_x(Length::Fill)
            .height(Length::Fill)
            .width(Length::Fill)
            .style(theme::container::background)
            .into()
        }
    }
}

pub fn layout<'a>(
    progress: (usize, usize),
    email: Option<&'a str>,
    breadcrumb: &[String],
    content: impl Into<Element<'a, Msg>>,
    padding_left: bool,
    previous_message: Option<Msg>,
) -> Element<'a, Msg> {
    layout_inner(
        progress,
        email,
        false,
        breadcrumb,
        LayoutContent::Scrollable(content.into()),
        padding_left,
        previous_message,
    )
}

/// Layout variant with fixed header content and a scrollable list section.
/// The header_content stays fixed at top, only the list_content scrolls.
/// An optional footer_content can be placed below the scrollable area.
#[allow(clippy::too_many_arguments)]
pub fn layout_with_scrollable_list<'a>(
    progress: (usize, usize),
    email: Option<&'a str>,
    is_ws_admin: bool,
    breadcrumb: &[String],
    header_content: impl Into<Element<'a, Msg>>,
    list_content: impl Into<Element<'a, Msg>>,
    footer_content: Option<Element<'a, Msg>>,
    padding_left: bool,
    previous_message: Option<Msg>,
) -> Element<'a, Msg> {
    layout_inner(
        progress,
        email,
        is_ws_admin,
        breadcrumb,
        LayoutContent::ScrollableList {
            header: header_content.into(),
            list: list_content.into(),
            footer: footer_content,
        },
        padding_left,
        previous_message,
    )
}

// NOTE: content MUST have width and height to Length::Fill
pub fn menu_entry(content: Row<'_, Msg>, message: Option<Msg>) -> Container<'_, Msg> {
    let content = content.width(Length::Fill).height(Length::Fill);
    let card = button::list_entry(content, None, EntryWidth::Standard, message);
    container(card)
        .width(MENU_ENTRY_WIDTH)
        .height(MENU_ENTRY_HEIGHT)
}

fn delete_btn(message: Option<Msg>) -> Button<'static, Msg> {
    icon_btn(icon::trash_icon(), message)
}

pub fn menu_key_entry(
    key: &ws_business::Key,
    last_edit_info: Option<String>,
    pill: Element<'static, Msg>,
    msg: Option<Msg>,
) -> Element<'static, Msg> {
    let identity_str = short_email(&key.identity.to_string(), 40);
    let subtitle = if !identity_str.is_empty() {
        Some(identity_str)
    } else {
        last_edit_info
    };

    let alias = truncate(&key.alias, 25);

    list::entry_key(
        entry_key_kind(&key.key_type),
        alias,
        subtitle,
        Some(pill),
        msg,
    )
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

/// Optional centered search bar inside a select list view.
pub struct SelectSearch<'a> {
    pub placeholder: &'static str,
    pub value: &'a str,
    pub on_change: fn(String) -> Msg,
}

/// Standard "pick one from a list" page used by org_select and wallet_select.
pub struct SelectListView<'a> {
    pub progress: (usize, usize),
    pub email: &'a str,
    pub is_ws_admin: bool,
    pub breadcrumb: Vec<String>,
    pub title: String,
    pub search: Option<SelectSearch<'a>>,
    pub list: Column<'static, Msg>,
    pub previous_message: Option<Msg>,
}

pub fn select_list_view(cfg: SelectListView<'_>) -> Element<'_, Msg> {
    let title_row = Row::new()
        .push(Space::with_width(Length::Fill))
        .push(text::h2(cfg.title))
        .push(Space::with_width(Length::Fill));

    let mut header = Column::new()
        .push(title_row)
        .push(Space::with_height(30))
        .spacing(10)
        .align_x(Alignment::Center)
        .padding([0, 20]);

    if let Some(search) = cfg.search {
        let value = form::Value {
            value: search.value.to_string(),
            warning: None,
            valid: true,
        };
        let search_form = form::Form::new_trimmed(search.placeholder, &value, search.on_change)
            .size(16)
            .padding(10);
        let search_container = Container::new(search_form).align_x(Alignment::Center);
        header = header.push(search_container);
    }

    let list = cfg.list.push(Space::with_height(50));

    layout_with_scrollable_list(
        cfg.progress,
        Some(cfg.email),
        cfg.is_ws_admin,
        &cfg.breadcrumb,
        header,
        list,
        None,
        true,
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

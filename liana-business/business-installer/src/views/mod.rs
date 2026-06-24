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
    widget::{column, container, row, rule, Space},
    Alignment, Length, Padding,
};
use liana_ui::{
    component::{
        button::{self, EntryWidth},
        form, list, scrollable,
        text::{self, short_email, truncate},
    },
    icon, theme,
    widget::*,
};
use std::fmt::Display;
use uuid::Uuid;

pub const INSTALLER_STEPS: usize = 7;
pub const MENU_ENTRY_WIDTH: u32 = 600;
pub const MENU_ENTRY_HEIGHT: u32 = 100;
const TOP_STRIP_HEIGHT: u32 = 44;

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

const CONTENT_TOP_SPACING: u32 = 72;
const SCREEN_INTRO_SUB_WIDTH: u32 = 620;

fn breadcrumb_header<'a>(segments: &[String]) -> Element<'a, Msg> {
    let mut row = Row::new().spacing(10).align_y(Alignment::Center);

    for (i, segment) in segments.iter().enumerate() {
        if i > 0 {
            row = row.push(list::breadcrumb_chevron());
        }
        row = row.push(if i + 1 == segments.len() {
            text::new::h3_semi(segment).style(theme::text::primary)
        } else {
            text::new::h3(segment).style(theme::text::muted)
        });
    }

    row.wrap().into()
}

enum LayoutContent<'a> {
    Scrollable(Element<'a, Msg>),
    ScrollableList {
        header: Element<'a, Msg>,
        list: Element<'a, Msg>,
        pinned: Option<Element<'a, Msg>>,
        footer: Option<Element<'a, Msg>>,
    },
}

fn thin_separator<'a>() -> Element<'a, Msg> {
    rule::horizontal(1).style(theme::rule::separator).into()
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

    let mut user = row![].spacing(12).align_y(Alignment::Center);

    if is_ws_admin {
        user = user.push(liana_ui::component::pill::ws_admin());
    }

    if let Some(e) = email {
        user = user
            .push(icon::person_icon().size(16).style(theme::text::tertiary))
            .push(text::new::caption(e).style(theme::text::accent));
    }

    let top_strip = Container::new(row![Space::fill_width(), user])
        .height(TOP_STRIP_HEIGHT)
        .width(Length::Fill)
        .padding([0, 28])
        .align_y(Alignment::Center)
        .style(theme::container::top_bar);

    let progress = if progress.1 > 0 {
        Element::from(
            row![
                text::new::caption(progress.0.to_string()).style(theme::text::accent),
                text::new::caption(format!(" | {}", progress.1))
            ]
            .align_y(Alignment::Center),
        )
    } else {
        Element::from(Space::fill_width())
    };
    let header = Container::new(
        row![
            if has_left_button {
                Container::new(left_button)
                    .center_x(Length::FillPortion(2))
                    .into()
            } else {
                Element::from(Space::with_width(Length::FillPortion(2)))
            },
            Container::new(breadcrumb_header(breadcrumb)).width(Length::FillPortion(8)),
            Container::new(progress).center_x(Length::FillPortion(2))
        ]
        .align_y(Alignment::Center),
    )
    .padding([20, 0]);

    let layout_header = column![top_strip, thin_separator(), header];

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
                    Container::new(column![
                        Space::with_height(CONTENT_TOP_SPACING as f32,),
                        inner
                    ])
                    .width(Length::FillPortion(fill_portion)),
                )
                .push_maybe(right_spacer());

            Container::new(
                column![
                    layout_header,
                    scrollable::vertical(content_row).height(Length::Fill)
                ]
                .width(Length::Fill)
                .height(Length::Fill),
            )
            .center_x(Length::Fill)
            .height(Length::Fill)
            .width(Length::Fill)
            .style(theme::container::background)
            .into()
        }
        LayoutContent::ScrollableList {
            header: header_content,
            list,
            pinned,
            footer,
        } => {
            let header_area = row![
                Space::with_width(Length::FillPortion(2)),
                Container::new(header_content)
                    .width(Length::FillPortion(fill_portion))
                    .padding(Padding {
                        top: 22.0,
                        bottom: 14.0,
                        ..Padding::ZERO
                    }),
                right_spacer()
            ];

            let list_area = Row::new()
                .push(Space::with_width(Length::FillPortion(2)))
                .push(
                    Container::new(scrollable::vertical(list).height(Length::Fill))
                        .width(Length::FillPortion(fill_portion))
                        .height(Length::Fill)
                        .align_x(Alignment::Center),
                )
                .push_maybe(right_spacer())
                .height(Length::Fill);

            let pinned_area: Option<Row<'a, Msg>> = pinned.map(|p| {
                row![
                    Space::with_width(Length::FillPortion(2)),
                    Container::new(p).width(Length::FillPortion(fill_portion)),
                    right_spacer()
                ]
            });
            let footer_area: Option<Row<'a, Msg>> = footer.map(|f| {
                Row::new()
                    .push(Space::with_width(Length::FillPortion(2)))
                    .push(Container::new(f).width(Length::FillPortion(fill_portion)))
                    .push_maybe(right_spacer())
            });

            Container::new(
                column![
                    layout_header,
                    header_area,
                    list_area,
                    pinned_area,
                    footer_area
                ]
                .width(Length::Fill)
                .height(Length::Fill),
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
/// Optional pinned_content and footer_content can be placed below the scrollable area.
#[allow(clippy::too_many_arguments)]
pub fn layout_with_scrollable_list<'a>(
    progress: (usize, usize),
    email: Option<&'a str>,
    is_ws_admin: bool,
    breadcrumb: &[String],
    header_content: impl Into<Element<'a, Msg>>,
    list_content: impl Into<Element<'a, Msg>>,
    pinned_content: Option<Element<'a, Msg>>,
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
            pinned: pinned_content,
            footer: footer_content,
        },
        padding_left,
        previous_message,
    )
}

pub fn screen_intro<'a, M: 'a>(
    title: impl Display + 'a,
    sub: Option<Element<'a, M>>,
) -> Element<'a, M> {
    column![text::new::d3(title), sub]
        .align_x(Alignment::Center)
        .padding(Padding {
            bottom: 24.0,
            ..Padding::ZERO
        })
        .into()
}

pub fn intro_description<'a, M: 'a>(text: &'a str) -> Element<'a, M> {
    Container::new(text::new::caption(text).style(theme::text::secondary))
        .width(Length::Shrink)
        .max_width(SCREEN_INTRO_SUB_WIDTH)
        .align_x(Alignment::Center)
        .padding(Padding {
            top: 6.0,
            ..Padding::ZERO
        })
        .into()
}

pub fn intro_prompt<'a, M: 'a>(prompt: &'a str, accent: Option<&'a str>) -> Element<'a, M> {
    let accent = accent.map(|accent| text::new::h3_semi(accent).style(theme::text::accent));
    Container::new(row![text::new::h3_semi(prompt), accent])
        .center_x(Length::Fill)
        .padding(Padding {
            top: 20.0,
            ..Padding::ZERO
        })
        .into()
}

// NOTE: content MUST have width and height to Length::Fill
pub fn menu_entry(content: Row<'_, Msg>, message: Option<Msg>) -> Container<'_, Msg> {
    let content = content.width(Length::Fill).height(Length::Fill);
    let card = button::list_entry(content, None, EntryWidth::Standard, message);
    container(card)
        .width(MENU_ENTRY_WIDTH)
        .height(MENU_ENTRY_HEIGHT)
}

pub fn menu_key_entry(
    key: &ws_business::Key,
    signer: String,
    kind_pill: Element<'static, Msg>,
    trailing: Element<'static, Msg>,
    msg: Option<Msg>,
) -> Element<'static, Msg> {
    let alias = truncate(&key.alias, 25);

    list::entry_key(
        entry_key_kind(&key.key_type),
        alias,
        kind_pill,
        short_email(&signer, 40),
        Some(trailing),
        msg,
    )
}

pub(crate) const KEY_KIND_LABEL: [(ws_business::KeyType, &str); 4] = [
    (ws_business::KeyType::Internal, "Internal"),
    (ws_business::KeyType::External, "External"),
    (ws_business::KeyType::Cosigner, "Cosigner"),
    (ws_business::KeyType::SafetyNet, "Safety Net"),
];

pub(crate) fn key_kind_label(key_type: &ws_business::KeyType) -> &'static str {
    KEY_KIND_LABEL
        .iter()
        .find_map(|(kind, label)| (kind == key_type).then_some(*label))
        .expect("every key type must have a label")
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
    let mut header = column![screen_intro(cfg.title, None)]
        .spacing(10)
        .align_x(Alignment::Center)
        .padding(20);

    if let Some(search) = cfg.search {
        let value = form::Value {
            value: search.value.to_string(),
            warning: None,
            valid: true,
        };
        let search_form = form::Form::new_trimmed(search.placeholder, &value, search.on_change)
            .size(16)
            .padding(10);
        let search_container = Container::new(search_form)
            .width(500)
            .align_x(Alignment::Center);
        header = header.push(search_container).push(Space::with_height(10));
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

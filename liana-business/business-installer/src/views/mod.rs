pub mod keys;
pub mod loading;
pub mod login;
pub mod modals;
pub mod org_select;
pub mod paths;
pub mod registration;
pub mod template_builder;
pub mod wallet_edit;
pub mod wallet_select;
pub mod xpub;

pub use keys::keys_view;
use liana_connect::ws_business::{self, UserRole};
use liana_ui::component::{button::btn_breadcrumb_previous, pill, text::capitalize_first};
pub use loading::loading_view;
pub use login::login_view;
pub use org_select::org_select_view;
pub use registration::registration_view;
pub use template_builder::template_builder_view;
pub use wallet_select::wallet_select_view;
pub use xpub::xpub_view;

use crate::{
    backend::Backend,
    state::{message::Msg, State},
};
use iced::{
    widget::{column, container, row, rule, Space},
    Alignment, Length,
};
use liana_ui::{
    color,
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
pub const MENU_ENTRY_HEIGHT: u32 = 100;
const CONTENT_WIDTH: f32 = button::STANDARD_ENTRY_WIDTH;
const USERBAR_HEIGHT: u32 = 44;
const MAX_SCROLL_HEIGHT: u32 = 500;
const HEADER_HEIGHT: u32 = 88;

fn format_last_edit_info_string_helper(
    last_edited: Option<u64>,
    last_editor: Option<Uuid>,
    state: &State,
    current_user_email_lower: &str,
) -> Option<(String, String)> {
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
                    short_email(&user.email, 25)
                }
            })
        })
        .map(|name| format!(" by {name}"))
        .unwrap_or_default();
    let relative_time = state.app.format_relative_time(timestamp);
    let edited = format!("Edited{editor_str} ");
    Some((edited, relative_time))
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
    format_last_edit_info_string_helper(last_edited, last_editor, state, current_user_email_lower)
        .map(|(a, b)| {
            row![
                text::new::caption(a).style(theme::text::secondary),
                text::new::caption(b).style(theme::text::secondary)
            ]
            .wrap()
            .into()
        })
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
    let mut row = row![].spacing(10).align_y(Alignment::Center);

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
        header: Option<Element<'a, Msg>>,
        list: Element<'a, Msg>,
        pinned: Option<Element<'a, Msg>>,
        footer: Option<Element<'a, Msg>>,
    },
}

fn thin_separator<'a>() -> Container<'a, Msg> {
    Container::new(rule::horizontal(1).style(theme::rule::separator))
}

fn user_bar<'a>(is_ws_admin: bool, email: Option<&'a str>) -> Element<'a, Msg> {
    let ws_admin_pill = is_ws_admin.then_some(pill::ws_admin());
    let user = email.map(|e| {
        row![
            icon::person_icon().size(16).style(theme::text::tertiary),
            text::new::caption(e).style(theme::text::accent)
        ]
        .spacing(12)
    });
    let user_bar = row![Space::fill_width(), ws_admin_pill, user]
        .spacing(12)
        .align_y(Alignment::Center);

    Container::new(user_bar)
        .height(USERBAR_HEIGHT)
        .padding([0, 28])
        .align_y(Alignment::Center)
        .style(theme::container::top_bar)
        .into()
}

fn step_dots<'a>((step, total): (usize, usize)) -> Element<'a, Msg> {
    let mut dots = row![].spacing(4).align_y(Alignment::Center);
    for i in 0..total {
        let filled = i < step;
        let width = if i + 1 == step { 20.0 } else { 8.0 };
        dots = dots.push(
            Container::new(Space::new())
                .width(width)
                .height(8)
                .style(if filled {
                    theme::container::step_dot_filled
                } else {
                    theme::container::step_dot_track
                }),
        );
    }
    dots.push(Space::with_width(4))
        .push(
            text::new::small_caption(format!("{step}/{total}"))
                .style(move |_: &theme::Theme| theme::text::custom(color::BUSINESS_STEP_LABEL)),
        )
        .into()
}

fn header<'a>(
    breadcrumb: &[String],
    msg: Option<Msg>,
    has_left_button: bool,
    progress: (usize, usize),
) -> Element<'a, Msg> {
    let left_button = btn_breadcrumb_previous(msg);

    let progress = if progress.1 > 0 {
        step_dots(progress)
    } else {
        row![].into()
    };
    let left_width = 200;
    let left = if has_left_button {
        Container::new(left_button).center_x(left_width)
    } else {
        Container::new(Space::with_width(left_width))
    };

    row![
        left,
        Container::new(breadcrumb_header(breadcrumb)).width(Length::FillPortion(8)),
        Container::new(progress).center_x(Length::FillPortion(2)),
    ]
    .align_y(Alignment::Center)
    .height(HEADER_HEIGHT)
    .into()
}

fn layout_inner<'a>(
    progress: (usize, usize),
    email: Option<&'a str>,
    is_ws_admin: bool,
    breadcrumb: &[String],
    content: LayoutContent<'a>,
    previous_message: Option<Msg>,
) -> Element<'a, Msg> {
    let user_bar = user_bar(is_ws_admin, email);

    let has_left_button = previous_message.is_some() || email.is_some();

    let msg = if let Some(msg) = previous_message {
        Some(msg)
    } else if email.is_some() {
        Some(Msg::Disconnect)
    } else {
        None
    };
    let header = header(breadcrumb, msg, has_left_button, progress);

    let top = column![user_bar, thin_separator(), header];

    let content = match content {
        LayoutContent::Scrollable(inner) => {
            let content_body = Container::new(column![
                Space::with_height(CONTENT_TOP_SPACING as f32),
                inner,
            ])
            .width(CONTENT_WIDTH);

            column![
                top,
                scrollable::vertical(content_body)
                    .height(Length::Fill)
                    .width(Length::Shrink),
            ]
        }
        LayoutContent::ScrollableList {
            header,
            list,
            pinned,
            footer,
        } => {
            let header = Container::new(header).center_x(Length::Fill);
            let list_body =
                Container::new(scrollable::vertical(list)).max_height(MAX_SCROLL_HEIGHT);
            let footer = footer.map(|f| {
                column![
                    thin_separator(),
                    Space::with_height(8),
                    f,
                    Space::with_height(8),
                ]
            });

            let body = column![header, list_body, pinned, Space::fill_height(), footer,]
                .spacing(12)
                .width(CONTENT_WIDTH);

            column![top, body].width(Length::Fill)
        }
    }
    .align_x(Alignment::Center)
    .width(Length::Fill)
    .height(Length::Fill);

    Container::new(content)
        .center_x(Length::Fill)
        .height(Length::Fill)
        .style(theme::container::background)
        .into()
}

pub fn layout<'a>(
    progress: (usize, usize),
    email: Option<&'a str>,
    breadcrumb: &[String],
    content: impl Into<Element<'a, Msg>>,
    previous_message: Option<Msg>,
) -> Element<'a, Msg> {
    layout_inner(
        progress,
        email,
        false,
        breadcrumb,
        LayoutContent::Scrollable(content.into()),
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
    header_content: Option<Element<'a, Msg>>,
    list_content: impl Into<Element<'a, Msg>>,
    pinned_content: Option<Element<'a, Msg>>,
    footer_content: Option<Element<'a, Msg>>,
    previous_message: Option<Msg>,
) -> Element<'a, Msg> {
    layout_inner(
        progress,
        email,
        is_ws_admin,
        breadcrumb,
        LayoutContent::ScrollableList {
            header: header_content,
            list: list_content.into(),
            pinned: pinned_content,
            footer: footer_content,
        },
        previous_message,
    )
}

pub fn screen_intro<'a, M: 'a>(
    title: impl Display + 'a,
    sub: Option<Element<'a, M>>,
    extra_spacing: bool,
) -> Element<'a, M> {
    let spacing = if extra_spacing { 60 } else { 15 };
    column![text::new::d3(title), sub]
        .align_x(Alignment::Center)
        .spacing(spacing)
        .into()
}

pub fn intro_description<'a, M: 'a>(text: &'a str) -> Element<'a, M> {
    Container::new(text::new::caption(text).style(theme::text::secondary))
        .width(Length::Shrink)
        .max_width(SCREEN_INTRO_SUB_WIDTH)
        .align_x(Alignment::Center)
        .into()
}

pub fn intro_prompt<'a, M: 'a>(prompt: &'a str, accent: Option<&'a str>) -> Element<'a, M> {
    let accent = accent.map(|accent| text::new::h3_semi(accent).style(theme::text::accent));
    Container::new(row![text::new::h3_semi(prompt), accent].wrap())
        .center_x(Length::Fill)
        .into()
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
    let mut header = column![screen_intro(cfg.title, None, false)]
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

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
pub use loading::loading_view;
pub use login::login_view;
pub use org_select::org_select_view;
pub use registration::registration_view;
pub use template_builder::template_builder_view;
pub use wallet_select::wallet_select_view;
pub use xpub::xpub_view;

use crate::{backend::Backend, state::message::Msg, state::State};
use iced::{
    widget::{container, scrollable, Space},
    Alignment, Length,
};
use liana_ui::{
    component::{
        button::{btn_flat, icon_btn, BtnWidth},
        card::clickable_card,
        text,
    },
    icon, theme,
    widget::*,
};
use uuid::Uuid;

pub const INSTALLER_STEPS: usize = 5;
pub const MENU_ENTRY_WIDTH: u16 = 600;
pub const ACCOUNT_ENTRY_WIDTH: u16 = MENU_ENTRY_WIDTH - 80;
pub const MENU_ENTRY_HEIGHT: u16 = 80;
const EMAIL_ROW_HEIGHT: u16 = 56;

/// Format last edit information as "Edited by [You|email] [relative_time]".
/// Returns None if `last_edited` is None.
pub fn format_last_edit_info(
    last_edited: Option<u64>,
    last_editor: Option<Uuid>,
    state: &State,
    current_user_email_lower: &str,
) -> Option<String> {
    last_edited.map(|ts| {
        let relative_time = state.app.format_relative_time(ts);
        let editor_name = last_editor
            .and_then(|editor_id| state.backend.get_user(editor_id))
            .map(|user| {
                if user.email.to_lowercase() == current_user_email_lower {
                    "You".to_string()
                } else {
                    user.email.clone()
                }
            })
            .unwrap_or_else(|| "Unknown".to_string());
        format!("Edited by {} {}", editor_name, relative_time)
    })
}

const EMAIL_HEADER_SPACER: u16 = 30;

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
    role_badge: Option<&'static str>,
    breadcrumb: &[String],
    content: LayoutContent<'a>,
    padding_left: bool,
    previous_message: Option<Msg>,
) -> Element<'a, Msg> {
    let icn = Some(icon::previous_icon());
    let has_left_button = previous_message.is_some() || email.is_some();
    let (txt, msg) = if let Some(msg) = previous_message {
        ("Previous", Some(msg))
    } else if email.is_some() {
        ("Disconnect", Some(Msg::Disconnect))
    } else {
        ("Previous", None)
    };

    let left_button = btn_flat(icn, txt, BtnWidth::L, msg);

    // Build the top-right row with optional role badge and email
    let mut email_row = Row::new()
        .push(Space::with_width(Length::Fill))
        .spacing(10)
        .align_y(Alignment::Center);

    if let Some(role) = role_badge {
        email_row = email_row.push(
            Container::new(text::caption(role))
                .padding([4, 12])
                .style(theme::pill::simple),
        );
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

            Container::new(scrollable(
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
                    Container::new(scrollable(list).height(Length::Fill))
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
    role_badge: Option<&'static str>,
    breadcrumb: &[String],
    content: impl Into<Element<'a, Msg>>,
    padding_left: bool,
    previous_message: Option<Msg>,
) -> Element<'a, Msg> {
    layout_inner(
        progress,
        email,
        role_badge,
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
    role_badge: Option<&'static str>,
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
        role_badge,
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
    let card = clickable_card(content, message);
    container(card)
        .width(MENU_ENTRY_WIDTH)
        .height(MENU_ENTRY_HEIGHT)
}

fn account_entry(content: Row<'_, Msg>, message: Option<Msg>) -> Container<'_, Msg> {
    menu_entry(content, message).width(ACCOUNT_ENTRY_WIDTH)
}

fn delete_btn(message: Option<Msg>) -> Button<'static, Msg> {
    icon_btn(icon::trash_icon(), message)
}

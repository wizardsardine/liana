//! Button galleries: every `liana_ui::theme::button::*` style across the four
//! button states, then every `liana_ui::component::button::*` constructor as a
//! real widget, enabled and (when it takes an optional message) disabled.
//! Pages are capped at [`MAX_ROWS`] so none of them scrolls.

use iced::{
    widget::{
        button::{Status, Style},
        column, container, row,
    },
    Alignment,
};
use liana_ui::{
    component::{
        button::{self as btn, BtnWidth, EntryWidth},
        list::{entry_accent, EntryAccent},
        text,
    },
    icon, theme,
    widget::{Column, Container, Element, Row},
};

use crate::debug::{debug_chrome, DebugMessage};

#[rustfmt::skip]
crate::debug_pages!(
    (ENTRY_THEMES_1, debug_chrome("Button themes 1/3", themes_table(THEMES_1))),
    (ENTRY_THEMES_2, debug_chrome("Button themes 2/3", themes_table(THEMES_2))),
    (ENTRY_THEMES_3, debug_chrome("Button themes 3/3", themes_table(THEMES_3))),
    (ENTRY_THEMED_1, debug_chrome("Button constructors: themed 1/2", constructors_table(themed_1()))),
    (ENTRY_THEMED_2, debug_chrome("Button constructors: themed 2/2", constructors_table(themed_2()))),
    (ENTRY_WIDTHS, debug_chrome("Button constructors: preset widths", constructors_table(widths()))),
    (ENTRY_FEERATE_LINKS, debug_chrome("Button constructors: feerate and links", constructors_table(feerate_links()))),
    (ENTRY_ACTIONS_1, debug_chrome("Button constructors: dialog actions 1/3", constructors_table(actions_1()))),
    (ENTRY_ACTIONS_2, debug_chrome("Button constructors: dialog actions 2/3", constructors_table(actions_2()))),
    (ENTRY_ACTIONS_3, debug_chrome("Button constructors: dialog actions 3/3", constructors_table(actions_3()))),
    (ENTRY_NAVIGATION, debug_chrome("Button constructors: icons and navigation", constructors_table(navigation()))),
    (ENTRY_WALLET_1, debug_chrome("Button constructors: wallet and spend 1/2", constructors_table(wallet_1()))),
    (ENTRY_WALLET_2, debug_chrome("Button constructors: wallet and spend 2/2", constructors_table(wallet_2()))),
    (ENTRY_INSTALLER_1, debug_chrome("Button constructors: installer and business 1/2", constructors_table(installer_1()))),
    (ENTRY_INSTALLER_2, debug_chrome("Button constructors: installer and business 2/2", constructors_table(installer_2()))),
    (ENTRY_ENTRIES, debug_chrome("Button constructors: entries and devices", constructors_table(entries()))),
);

const CONSTRUCTOR_PREFIX: &str = "liana_ui::component::button::";
/// Rows per page, so no page needs to scroll.
const MAX_ROWS: usize = 12;
const NAME_W: u32 = 500;
const STATE_W: u32 = 140;
const PATH_W: u32 = 560;
const SAMPLE_W: u32 = 640;
const SPACING: u32 = 12;
const ROUND_ICON_RADIUS: f32 = 20.0;

type StyleFn = fn(&theme::Theme, Status) -> Style;
type Sample = Element<'static, DebugMessage>;

const STATES: [(Status, &str); 4] = [
    (Status::Active, "Active"),
    (Status::Hovered, "Hovered"),
    (Status::Pressed, "Pressed"),
    (Status::Disabled, "Disabled"),
];

#[rustfmt::skip]
const THEMES_1: &[(&str, StyleFn)] = &[
    ("liana_ui::theme::button::primary",                      theme::button::primary),
    ("liana_ui::theme::button::secondary",                    theme::button::secondary),
    ("liana_ui::theme::button::tertiary",                     theme::button::tertiary),
    ("liana_ui::theme::button::destructive",                  theme::button::destructive),
    ("liana_ui::theme::button::feerate",                      theme::button::feerate),
    ("liana_ui::theme::button::feerate_unselected",           theme::button::feerate_unselected),
    ("liana_ui::theme::button::container",                    theme::button::container),
    ("liana_ui::theme::button::container_border",             theme::button::container_border),
    ("liana_ui::theme::button::menu",                         theme::button::menu),
    ("liana_ui::theme::button::menu_pressed",                 theme::button::menu_pressed),
];

#[rustfmt::skip]
const THEMES_2: &[(&str, StyleFn)] = &[
    ("liana_ui::theme::button::tab_menu",                     theme::button::tab_menu),
    ("liana_ui::theme::button::tab_menu_bottom",              theme::button::tab_menu_bottom),
    ("liana_ui::theme::button::tab",                          theme::button::tab),
    ("liana_ui::theme::button::tab_active",                   theme::button::tab_active),
    ("liana_ui::theme::button::transparent",                  theme::button::transparent),
    ("liana_ui::theme::button::transparent_primary_text",     theme::button::transparent_primary_text),
    ("liana_ui::theme::button::transparent_border",           theme::button::transparent_border),
    ("liana_ui::theme::button::remove",                       theme::button::remove),
    ("liana_ui::theme::button::link",                         theme::button::link),
    ("liana_ui::theme::button::link_subtle",                  theme::button::link_subtle),
];

#[rustfmt::skip]
const THEMES_3: &[(&str, StyleFn)] = &[
    ("liana_ui::theme::button::auxiliary",                    theme::button::auxiliary),
    ("liana_ui::theme::button::breadcrumb",                   theme::button::breadcrumb),
    ("liana_ui::theme::button::list_entry",                   theme::button::list_entry),
    ("liana_ui::theme::button::list_entry_unfolded",          theme::button::list_entry_unfolded),
    ("liana_ui::theme::button::signing_devices",              theme::button::signing_devices),
    ("liana_ui::theme::button::signing_devices_non_clickable", theme::button::signing_devices_non_clickable),
    ("liana_ui::theme::button::optional_section",             theme::button::optional_section),
    ("liana_ui::theme::button::round_icon_btn(_, _, 20.0)",   |t, s| theme::button::round_icon_btn(t, s, ROUND_ICON_RADIUS)),
    ("liana_ui::component::button::list_entry_style(_, _, None, true)",  |t, s| btn::list_entry_style(t, s, None, true)),
    ("liana_ui::component::button::list_entry_style(_, _, None, false)", |t, s| btn::list_entry_style(t, s, None, false)),
];

/// A label styled like a button in `status`, so all four states show at once.
fn fake_button(style: StyleFn, status: Status) -> Container<'static, DebugMessage> {
    Container::new(text::new::caption("Sample"))
        .padding(10)
        .style(move |theme: &theme::Theme| {
            let s = style(theme, status);
            container::Style {
                text_color: Some(s.text_color),
                background: s.background,
                border: s.border,
                shadow: s.shadow,
                ..Default::default()
            }
        })
}

fn header_cell(label: &'static str, width: u32) -> Container<'static, DebugMessage> {
    Container::new(text::new::b5_bold(label)).center_x(width)
}

fn themes_table(themes: &'static [(&'static str, StyleFn)]) -> Column<'static, DebugMessage> {
    assert!(themes.len() <= MAX_ROWS);
    let header = STATES.iter().fold(
        row![header_cell("Theme", NAME_W)].spacing(SPACING),
        |header, (_, label)| header.push(header_cell(label, STATE_W)),
    );
    themes
        .iter()
        .fold(column![header].spacing(SPACING), |col, (name, style)| {
            let name = text::new::caption(*name).width(NAME_W);
            let cells = STATES.iter().fold(
                row![name].spacing(SPACING).align_y(Alignment::Center),
                |cells, (status, _)| {
                    cells.push(Container::new(fake_button(*style, *status)).center_x(STATE_W))
                },
            );
            col.push(cells)
        })
}

struct Entry {
    call: &'static str,
    enabled: Sample,
    disabled: Option<Sample>,
}

/// A constructor taking an optional message, shown with and without it.
fn opt<E: Into<Sample>>(call: &'static str, build: impl Fn(Option<DebugMessage>) -> E) -> Entry {
    Entry {
        call,
        enabled: build(Some(())).into(),
        disabled: Some(build(None).into()),
    }
}

/// A constructor whose message is mandatory, shown enabled only.
fn req<E: Into<Sample>>(call: &'static str, build: impl FnOnce() -> E) -> Entry {
    Entry {
        call,
        enabled: build().into(),
        disabled: None,
    }
}

fn entry_row(entry: Entry) -> Row<'static, DebugMessage> {
    let path = text::new::caption(format!("{CONSTRUCTOR_PREFIX}{}", entry.call)).width(PATH_W);
    let enabled = Container::new(entry.enabled).center_x(SAMPLE_W);
    let disabled = entry
        .disabled
        .map(|sample| Container::new(sample).center_x(SAMPLE_W));
    row![path, enabled, disabled]
        .spacing(SPACING)
        .align_y(Alignment::Center)
}

fn constructors_table(entries: Vec<Entry>) -> Column<'static, DebugMessage> {
    assert!(entries.len() <= MAX_ROWS);
    let header = row![
        header_cell("Constructor", PATH_W),
        header_cell("Some(msg)", SAMPLE_W),
        header_cell("None", SAMPLE_W),
    ]
    .spacing(SPACING);
    entries
        .into_iter()
        .fold(column![header].spacing(SPACING), |col, entry| {
            col.push(entry_row(entry))
        })
}

#[rustfmt::skip]
fn themed_1() -> Vec<Entry> {
    vec![
        opt("menu(None, \"Item\", false)",                              |m| btn::menu(None, "Item", false).on_press_maybe(m)),
        opt("menu(Some(icon), \"Item\", true)",                         |m| btn::menu(Some(icon::wallet_icon()), "Item", true).on_press_maybe(m)),
        opt("menu_active(None, \"Item\", false)",                       |m| btn::menu_active(None, "Item", false).on_press_maybe(m)),
        opt("menu_active(Some(icon), \"Item\", true)",                  |m| btn::menu_active(Some(icon::wallet_icon()), "Item", true).on_press_maybe(m)),
        opt("menu_small(icon)",                                         |m| btn::menu_small(icon::wallet_icon()).on_press_maybe(m)),
        opt("menu_active_small(icon)",                                  |m| btn::menu_active_small(icon::wallet_icon()).on_press_maybe(m)),
        opt("button_with_theme(None, \"Sample\", primary, false)",      |m| btn::button_with_theme(None, "Sample", theme::button::primary, false).on_press_maybe(m)),
        opt("button_with_theme(Some(icon), \"Sample\", primary, true)", |m| btn::button_with_theme(Some(icon::plus_icon()), "Sample", theme::button::primary, true).on_press_maybe(m)),
        opt("button_compact(\"Sample\", secondary, msg)",               |m| btn::button_compact("Sample", theme::button::secondary, m)),
        opt("primary(None, \"Sample\")",                                |m| btn::primary(None, "Sample").on_press_maybe(m)),
        opt("secondary(None, \"Sample\")",                              |m| btn::secondary(None, "Sample").on_press_maybe(m)),
        opt("tertiary(None, \"Sample\")",                               |m| btn::tertiary(None, "Sample").on_press_maybe(m)),
    ]
}

#[rustfmt::skip]
fn themed_2() -> Vec<Entry> {
    vec![
        opt("destructive(None, \"Sample\")",              |m| btn::destructive(None, "Sample").on_press_maybe(m)),
        opt("alert(None, \"Sample\")",                    |m| btn::alert(None, "Sample").on_press_maybe(m)),
        opt("transparent(None, \"Sample\")",              |m| btn::transparent(None, "Sample").on_press_maybe(m)),
        opt("transparent_primary_text(None, \"Sample\")", |m| btn::transparent_primary_text(None, "Sample").on_press_maybe(m)),
        opt("transparent_border(None, \"Sample\")",       |m| btn::transparent_border(None, "Sample").on_press_maybe(m)),
        opt("flat(None, \"Sample\")",                     |m| btn::flat(None, "Sample").on_press_maybe(m)),
        opt("border(None, \"Sample\")",                   |m| btn::border(None, "Sample").on_press_maybe(m)),
        opt("link(None, \"Sample\")",                     |m| btn::link(None, "Sample").on_press_maybe(m)),
        opt("auxiliary(Some(icon), \"Sample\", msg)",     |m| btn::auxiliary(Some(icon::plus_icon()), "Sample", m)),
        opt("breadcrumb(\"Sample\")",                     |m| btn::breadcrumb("Sample").on_press_maybe(m)),
        opt("icon_btn(icon, msg)",                        |m| btn::icon_btn(icon::tooltip_icon(), m)),
        opt("clickable_icon_with_size(icon, msg, 26)",    |m| btn::clickable_icon_with_size(icon::edit_icon(), m, 26)),
    ]
}

#[rustfmt::skip]
fn widths() -> Vec<Entry> {
    vec![
        opt("btn_primary(None, \"S\", BtnWidth::S, msg)",           |m| btn::btn_primary(None, "S", BtnWidth::S, m)),
        opt("btn_primary(None, \"M\", BtnWidth::M, msg)",           |m| btn::btn_primary(None, "M", BtnWidth::M, m)),
        opt("btn_primary(None, \"L\", BtnWidth::L, msg)",           |m| btn::btn_primary(None, "L", BtnWidth::L, m)),
        opt("btn_primary(None, \"Modal\", BtnWidth::Modal, msg)",   |m| btn::btn_primary(None, "Modal", BtnWidth::Modal, m)),
        opt("btn_primary(None, \"XL\", BtnWidth::XL, msg)",         |m| btn::btn_primary(None, "XL", BtnWidth::XL, m)),
        opt("btn_primary(None, \"XXL\", BtnWidth::XXL, msg)",       |m| btn::btn_primary(None, "XXL", BtnWidth::XXL, m)),
        opt("btn_primary(None, \"Auto\", BtnWidth::Auto, msg)",     |m| btn::btn_primary(None, "Auto", BtnWidth::Auto, m)),
        opt("btn_primary(None, \"Fill\", BtnWidth::Fill, msg)",     |m| btn::btn_primary(None, "Fill", BtnWidth::Fill, m)),
        opt("btn_secondary(Some(icon), \"M\", BtnWidth::M, msg)",   |m| btn::btn_secondary(Some(icon::plus_icon()), "M", BtnWidth::M, m)),
        opt("btn_tertiary(Some(icon), \"M\", BtnWidth::M, msg)",    |m| btn::btn_tertiary(Some(icon::plus_icon()), "M", BtnWidth::M, m)),
        opt("btn_destructive(Some(icon), \"M\", BtnWidth::M, msg)", |m| btn::btn_destructive(Some(icon::plus_icon()), "M", BtnWidth::M, m)),
        opt("btn_flat(Some(icon), \"M\", BtnWidth::M, msg)",        |m| btn::btn_flat(Some(icon::plus_icon()), "M", BtnWidth::M, m)),
    ]
}

#[rustfmt::skip]
fn feerate_links() -> Vec<Entry> {
    vec![
        opt("btn_low(false, msg)",                |m| btn::btn_low(false, m)),
        opt("btn_low(true, msg)",                 |m| btn::btn_low(true, m)),
        opt("btn_medium(true, msg)",              |m| btn::btn_medium(true, m)),
        opt("btn_high(true, msg)",                |m| btn::btn_high(true, m)),
        opt("subtle_link(\"Sample\", msg)",       |m| btn::subtle_link("Sample", m)),
        opt("subtle_link_small(\"Sample\", msg)", |m| btn::subtle_link_small("Sample", m)),
        opt("btn_help_link(\"Sample\", msg)",     |m| btn::btn_help_link("Sample", m)),
        req("btn_backend_options_help(msg)",      || btn::btn_backend_options_help(())),
        opt("btn_template_help(msg)",             btn::btn_template_help),
    ]
}

#[rustfmt::skip]
fn actions_1() -> Vec<Entry> {
    vec![
        opt("btn_save(msg, true)",  |m| btn::btn_save(m, true)),
        opt("btn_save(msg, false)", |m| btn::btn_save(m, false)),
        opt("btn_cancel(msg)",      btn::btn_cancel),
        opt("btn_ok(msg)",          btn::btn_ok),
        opt("btn_overwrite(msg)",   btn::btn_overwrite),
        opt("btn_ignore(msg)",      btn::btn_ignore),
        opt("btn_yes(msg)",         btn::btn_yes),
        opt("btn_no(msg)",          btn::btn_no),
        opt("btn_confirm(msg)",     btn::btn_confirm),
        opt("btn_accept(msg)",      btn::btn_accept),
        opt("btn_dismiss(msg)",     btn::btn_dismiss),
        opt("btn_retry(msg)",       btn::btn_retry),
    ]
}

#[rustfmt::skip]
fn actions_2() -> Vec<Entry> {
    vec![
        opt("btn_reload(msg)",    btn::btn_reload),
        opt("btn_clear(msg)",     btn::btn_clear),
        opt("btn_clear_all(msg)", btn::btn_clear_all),
        opt("btn_customize(msg)", btn::btn_customize),
        opt("btn_select(msg)",    btn::btn_select),
        opt("btn_skip(msg)",      btn::btn_skip),
        opt("btn_edit(msg)",      btn::btn_edit),
        opt("btn_set(msg)",       btn::btn_set),
        opt("btn_update(msg)",    btn::btn_update),
        opt("btn_delete(msg)",    btn::btn_delete),
        opt("btn_generate(msg)",  btn::btn_generate),
        opt("btn_unlock(msg)",    btn::btn_unlock),
    ]
}

#[rustfmt::skip]
fn actions_3() -> Vec<Entry> {
    vec![
        opt("btn_email_wizardsardine(msg)", btn::btn_email_wizardsardine),
        opt("btn_try_other_paths(msg)",     btn::btn_try_other_paths),
        req("btn_processing()",             btn::btn_processing),
        req("btn_see_more(false, msg)",     || btn::btn_see_more(false, ())),
        req("btn_see_more(true, msg)",      || btn::btn_see_more(true, ())),
        req("btn_see_more_details(msg)",    || btn::btn_see_more_details(())),
        opt("btn_export(msg)",              btn::btn_export),
        opt("btn_export_psbt(true, msg)",   |m| btn::btn_export_psbt(true, m)),
        opt("btn_export_psbt(false, msg)",  |m| btn::btn_export_psbt(false, m)),
        opt("btn_import(msg)",              btn::btn_import),
        opt("btn_new(msg)",                 btn::btn_new),
    ]
}

#[rustfmt::skip]
fn navigation() -> Vec<Entry> {
    vec![
        opt("btn_previous(msg)",                btn::btn_previous),
        opt("btn_next(msg)",                    btn::btn_next),
        opt("btn_breadcrumb_previous(msg)",     btn::btn_breadcrumb_previous),
        opt("btn_modal_close(msg)",             btn::btn_modal_close),
        req("btn_modal_previous(msg)",          || btn::btn_modal_previous(())),
        req("btn_chevron(false, msg)",          || btn::btn_chevron(false, ())),
        req("btn_chevron(true, msg)",           || btn::btn_chevron(true, ())),
        opt("btn_remove(msg)",                  btn::btn_remove),
        opt("btn_icon_edit(msg)",               btn::btn_icon_edit),
        opt("btn_paste_icon(msg)",              btn::btn_paste_icon),
        opt("btn_copy(msg)",                    btn::btn_copy),
        req("btn_mnemonic_word(\"word\", msg)", || btn::btn_mnemonic_word("word", ())),
    ]
}

#[rustfmt::skip]
fn wallet_1() -> Vec<Entry> {
    vec![
        opt("btn_sign(msg)",                  btn::btn_sign),
        opt("btn_broadcast(msg)",             btn::btn_broadcast),
        opt("btn_go_back_to_psbts(msg)",      btn::btn_go_back_to_psbts),
        opt("btn_add_payment(msg)",           btn::btn_add_payment),
        opt("btn_go_to_replacement(msg)",     btn::btn_go_to_replacement),
        opt("btn_bump_fee(msg)",              btn::btn_bump_fee),
        opt("btn_cancel_transaction(msg)",    btn::btn_cancel_transaction),
        opt("btn_go_to_rescan(msg)",          btn::btn_go_to_rescan),
        opt("btn_start_rescan(false, msg)",   |m| btn::btn_start_rescan(false, m)),
        opt("btn_start_rescan(true, msg)",    |m| btn::btn_start_rescan(true, m)),
        opt("btn_reset_timelock(msg, true)",  |m| btn::btn_reset_timelock(m, true)),
        opt("btn_reset_timelock(msg, false)", |m| btn::btn_reset_timelock(m, false)),
    ]
}

#[rustfmt::skip]
fn wallet_2() -> Vec<Entry> {
    vec![
        opt("btn_generate_address(msg)",          btn::btn_generate_address),
        req("btn_show_qr(msg)",                   || btn::btn_show_qr(())),
        req("btn_show_qr_compact(msg)",           || btn::btn_show_qr_compact(())),
        req("btn_verify(msg)",                    || btn::btn_verify(())),
        req("btn_verify_compact(msg)",            || btn::btn_verify_compact(())),
        req("btn_register_on_device(msg)",        || btn::btn_register_on_device(())),
        opt("btn_backup_descriptor(msg, true)",   |m| btn::btn_backup_descriptor(m, true)),
        opt("btn_backup_descriptor(msg, false)",  |m| btn::btn_backup_descriptor(m, false)),
        req("btn_backup_encrypt_descriptor(msg)", || btn::btn_backup_encrypt_descriptor(())),
        opt("btn_check_connection(msg, true)",    |m| btn::btn_check_connection(m, true)),
        opt("btn_check_connection(msg, false)",   |m| btn::btn_check_connection(m, false)),
        opt("btn_delete_wallet(msg)",             btn::btn_delete_wallet),
    ]
}

#[rustfmt::skip]
fn installer_1() -> Vec<Entry> {
    vec![
        opt("btn_add_key(msg)",               btn::btn_add_key),
        opt("btn_add_recovery_path(msg)",     btn::btn_add_recovery_path),
        opt("btn_add_recovery_option(msg)",   btn::btn_add_recovery_option),
        opt("btn_add_safety_net(msg)",        btn::btn_add_safety_net),
        opt("btn_manage_keys(msg, true)",     |m| btn::btn_manage_keys(m, true)),
        opt("btn_manage_keys(msg, false)",    |m| btn::btn_manage_keys(m, false)),
        opt("btn_mark_keys_ready(msg)",       btn::btn_mark_keys_ready),
        opt("btn_edit_keys(msg)",             btn::btn_edit_keys),
        opt("btn_skip_registration(msg)",     btn::btn_skip_registration),
        opt("btn_add_wallet(msg)",            btn::btn_add_wallet),
        opt("btn_share_xpubs(msg)",           btn::btn_share_xpubs),
        req("btn_optional_section(row, msg)", || btn::btn_optional_section(row![text::new::caption("Optional section")], ())),
    ]
}

#[rustfmt::skip]
fn installer_2() -> Vec<Entry> {
    vec![
        opt("btn_send_token(msg)",            btn::btn_send_token),
        opt("btn_resend_token(msg)",          btn::btn_resend_token),
        opt("btn_change_email(msg)",          btn::btn_change_email),
        opt("btn_connect_another_email(msg)", btn::btn_connect_another_email),
        opt("btn_approve(msg)",               btn::btn_approve),
        opt("btn_send_for_approval(msg)",     btn::btn_send_for_approval),
        opt("btn_keep_changes(msg)",          btn::btn_keep_changes),
        opt("btn_send_invitation(msg)",       btn::btn_send_invitation),
    ]
}

#[rustfmt::skip]
fn entries() -> Vec<Entry> {
    let accent = entry_accent(EntryAccent::Success);
    vec![
        opt("list_entry(content, None, EntryWidth::Standard, msg)",                         |m| btn::list_entry(text::new::caption("Entry"), None, EntryWidth::Standard, m)),
        opt("list_entry(content, Some(accent), EntryWidth::Standard, msg)",                 |m| btn::list_entry(text::new::caption("Entry"), Some(accent), EntryWidth::Standard, m)),
        opt("list_entry_with_enabled(content, None, EntryWidth::Standard, false, msg)",     |m| btn::list_entry_with_enabled(text::new::caption("Entry"), None, EntryWidth::Standard, false, m)),
        opt("list_entry_with_state(content, None, EntryWidth::Standard, true, false, msg)", |m| btn::list_entry_with_state(text::new::caption("Entry"), None, EntryWidth::Standard, true, false, m)),
        req("list_entry_card(content, Some(accent), EntryWidth::Standard)",                 || btn::list_entry_card(text::new::caption("Entry"), Some(accent), EntryWidth::Standard)),
        opt("btn_show_qr_section(Some(\"tooltip\"), msg)",                                  |m| btn::btn_show_qr_section(Some("tooltip"), m)),
        opt("btn_show_qr_section(None, msg)",                                               |m| btn::btn_show_qr_section(None::<String>, m)),
        opt("device(content, msg)",                                                         |m| btn::device(text::new::caption("Device"), m)),
        opt("device_with_height(content, msg, 60)",                                         |m| btn::device_with_height(text::new::caption("Device"), m, 60)),
        opt("device_with_height_clickable(content, msg, None, false)",                      |m| btn::device_with_height_clickable(text::new::caption("Device"), m, None, false)),
    ]
}

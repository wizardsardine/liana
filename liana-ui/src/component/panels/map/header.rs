use iced::{
    alignment::{Horizontal, Vertical},
    widget::{column, row, text, Space},
    Length,
};
use liana_i18n::t;

use crate::{
    component::{amount::Amount, button, panels::map::separator, text::new},
    icon, theme,
    theme::Theme,
    widget::{Container, Element, SpaceExt},
};

const HEADER_HEIGHT: u32 = 72;

/// Header control pressed, mapped to a message by the caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderAction {
    Back,
    Undo,
    Redo,
    Shortcuts,
    ResetLayout,
    OtherWallets,
    ToggleArea,
    ToggleUnspent,
    ToggleSnap,
    AlignHorizontal,
    AlignVertical,
    ZoomOut,
    ZoomIn,
    Fit,
}

/// Map toolbar. `enabled` is false while the map is empty or loading, `align_count` is the
/// number of transactions the align buttons would move, `other_wallets` enables the other
/// wallets button.
#[allow(clippy::too_many_arguments)]
pub fn map_header<'a, M: Clone + 'a>(
    zoom: f32,
    enabled: bool,
    can_undo: bool,
    can_redo: bool,
    area_on: bool,
    unspent_on: bool,
    snap_on: bool,
    unspent_count: usize,
    unspent_total: &Amount,
    align_count: usize,
    other_wallets: bool,
    on_action: impl Fn(HeaderAction) -> M,
) -> Element<'a, M> {
    let msg = |action, active: bool| active.then(|| on_action(action));
    let can_align = enabled && align_count >= 2;

    let back = button::btn_modal_previous(on_action(HeaderAction::Back));
    let pin = icon::geo_alt_fill_icon()
        .size(24)
        .style(|theme: &Theme| text::Style {
            color: Some(theme.colors.general.accent),
        });
    let title = new::d3(t!("map-title"));

    let undo = button::btn_undo(msg(HeaderAction::Undo, enabled && can_undo));
    let redo = button::btn_redo(msg(HeaderAction::Redo, enabled && can_redo));
    let shortcuts = button::btn_shortcuts(msg(HeaderAction::Shortcuts, true));
    let reset = button::btn_reset_layout(msg(HeaderAction::ResetLayout, enabled));
    let other_wallets = button::btn_other_wallets(msg(HeaderAction::OtherWallets, other_wallets));

    let area = button::btn_select_area(area_on, msg(HeaderAction::ToggleArea, enabled));
    let unspent = button::btn_highlight_unspent(
        unspent_on,
        unspent_count,
        unspent_total,
        msg(HeaderAction::ToggleUnspent, enabled),
    );
    let snap = button::btn_snap(snap_on, msg(HeaderAction::ToggleSnap, enabled));

    let align_h = button::btn_align_h(align_count, msg(HeaderAction::AlignHorizontal, can_align));
    let align_v = button::btn_align_v(align_count, msg(HeaderAction::AlignVertical, can_align));

    let zoom_out = button::btn_zoom_out(msg(HeaderAction::ZoomOut, enabled));
    let zoom_label = new::b5_medium(format!("{}%", (zoom * 100.0).round() as u32))
        .style(theme::text::secondary)
        .width(52)
        .align_x(Horizontal::Center);
    let zoom_in = button::btn_zoom_in(msg(HeaderAction::ZoomIn, enabled));
    let fit = button::btn_fit(msg(HeaderAction::Fit, enabled));

    let sep = || Container::new(separator(1, 32)).padding([0, 6]);

    let toolbar = row![
        back,
        pin,
        title,
        Space::fill_width(),
        undo,
        redo,
        shortcuts,
        reset,
        sep(),
        other_wallets,
        sep(),
        area,
        unspent,
        snap,
        sep(),
        align_h,
        align_v,
        sep(),
        zoom_out,
        zoom_label,
        zoom_in,
        fit,
    ]
    .spacing(12)
    .align_y(Vertical::Center)
    .padding([0, 20])
    .height(HEADER_HEIGHT - 1);

    Container::new(column![toolbar, separator(Length::Fill, 1)])
        .style(theme::container::background)
        .width(Length::Fill)
        .into()
}

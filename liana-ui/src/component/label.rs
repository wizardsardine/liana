use std::fmt::Display;

use iced::{
    widget::{column, row, Space},
    Alignment,
};
use liana::label::Label;
use liana_i18n::t;

use crate::{
    component::{
        text::{apply, truncate, TextSpec},
        tooltip::tooltip_custom,
    },
    icon, theme,
    widget::{Element, SpaceExt},
};

use super::{
    button::{btn_cancel, btn_generate, btn_icon_edit, btn_save},
    form::{Form, Value},
    modal::modal_view,
};

pub fn display_label<'a, M: 'a>(
    label: &Label,
    spec: TextSpec,
    max_len: Option<usize>,
) -> Element<'a, M> {
    let (label, inherited_from) = match label {
        Label::Own(label) => (label.clone(), None),
        Label::Payment(label) => (label.clone(), Some(t!("label-inherited-payment"))),
        Label::Transaction(label) => (label.clone(), Some(t!("label-inherited-transaction"))),
        Label::Address(label) => (label.clone(), Some(t!("label-inherited-address"))),
        Label::Funding(label) => (label.clone(), Some(t!("label-inherited-funding"))),
        Label::None => (t!("common-no-label-parenthesized"), None),
    };
    let text: Element<'a, M> = match max_len {
        Some(max_len) if label.chars().count() > max_len => {
            let short = apply(truncate(&label, max_len), spec).style(theme::text::primary);
            tooltip_custom(
                apply(label, spec),
                short,
                iced::widget::tooltip::Position::Top,
            )
            .into()
        }
        _ => apply(label, spec).style(theme::text::primary).into(),
    };
    let info = inherited_from.map(|help| {
        let icon = icon::tooltip_icon().style(theme::text::secondary);
        tooltip_custom(
            iced::widget::text(help),
            icon,
            iced::widget::tooltip::Position::Top,
        )
    });
    row![text, info]
        .spacing(10)
        .align_y(Alignment::Center)
        .into()
}

pub fn editable_label<'a, M: 'a + Clone>(label: &Label, spec: TextSpec, msg: M) -> Element<'a, M> {
    let edit = btn_icon_edit(Some(msg));
    row![display_label(label, spec, None), edit]
        .spacing(10)
        .align_y(Alignment::Center)
        .into()
}

pub const LABEL_MAX_LENGTH: usize = 80;

pub fn edit_label_modal<'a, M: 'a + Clone, C>(
    title: impl Display,
    descr: impl Display,
    value: &Value<String>,
    on_change: C,
    confirm: M,
    close: M,
    is_new: bool,
) -> Element<'a, M>
where
    C: 'static + Fn(String) -> M,
{
    // An empty label is not an error (no warning shown), but it cannot be saved,
    // so the confirm button stays disabled until a label is entered.
    let confirm = (value.valid && !value.value.is_empty()).then_some(confirm);
    let input = Form::new(descr, value, on_change).warning(t!("label-invalid-length"));
    let input = match &confirm {
        Some(c) => input.on_submit(c.clone()),
        None => input,
    };
    let cancel = if is_new {
        None
    } else {
        Some(btn_cancel(Some(close.clone())))
    };
    let ok = if is_new {
        btn_generate(confirm)
    } else {
        btn_save(confirm, true)
    };
    let btn_row = row![Space::fill_width(), cancel, ok].spacing(12);
    let content = column![input, btn_row].spacing(28);
    modal_view(
        Some(title),
        None,
        Some(close),
        super::modal::ModalWidth::M,
        content,
    )
}

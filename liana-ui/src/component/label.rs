use std::fmt::Display;

use iced::{
    widget::{column, row, Space},
    Alignment,
};

use crate::{
    component::text::new,
    widget::{Element, SpaceExt},
};

use super::{
    button::{btn_cancel, btn_edit, btn_generate, btn_save},
    form::{Form, Value},
    modal::modal_view,
};

pub fn editable_label<'a, M: 'a + Clone>(label: impl Display, msg: M) -> Element<'a, M> {
    let mut label = label.to_string();
    if label.is_empty() {
        label = "(No label)".to_string();
    }
    let edit = btn_edit(Some(msg));
    let label = new::h2(label);
    row![label, edit]
        .spacing(10)
        .align_y(Alignment::Center)
        .into()
}

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
    let input = Form::new(descr, value, on_change);
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
        btn_save(confirm)
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

use std::fmt::Display;

use iced::{
    widget::{column, row, Space},
    Alignment,
};

use crate::{
    component::text::new::{self},
    widget::{Element, SpaceExt},
};

use super::{
    button::{btn_edit, btn_ok},
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
) -> Element<'a, M>
where
    C: 'static + Fn(String) -> M,
{
    let input = Form::new(descr, value, on_change).on_submit(confirm.clone());
    let btn_row = row![Space::fill_width(), btn_ok(Some(confirm))];
    let content = column![input, btn_row].spacing(28);
    modal_view(
        Some(title),
        None,
        Some(close),
        super::modal::ModalWidth::M,
        content,
    )
}

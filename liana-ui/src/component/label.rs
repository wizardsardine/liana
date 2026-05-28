use std::fmt::Display;

use iced::{widget::row, Alignment};

use crate::{
    component::{
        button,
        text::new::{self, H2_SPEC},
    },
    icon,
    widget::Element,
};

pub fn editable_label<'a, M: 'a + Clone>(label: impl Display, msg: M) -> Element<'a, M> {
    let mut label = label.to_string();
    if label.is_empty() {
        label = "(No label)".to_string();
    }
    let icon = icon::edit_icon().size(H2_SPEC.size.expect("size"));
    let edit = button::flat(Some(icon), "").on_press(msg);
    let label = new::h2(label);
    row![label, edit]
        .spacing(12)
        .padding(5)
        .align_y(Alignment::Center)
        .into()
}

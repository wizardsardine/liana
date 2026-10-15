use std::fmt::Display;

use iced::{
    widget::{checkbox, radio, row},
    Alignment, Pixels,
};

use crate::{
    component::text::new,
    theme,
    widget::{CheckBox, Element, Radio, Row, Toggler},
};

const LABEL_SPACING: u32 = 10;

const CHECKBOX_SIZE: f32 = 18.0;

#[derive(Debug, Clone, Copy)]
#[repr(u32)]
pub enum TogglerSize {
    Normal = 16,
    Large = 28,
}

impl From<TogglerSize> for Pixels {
    fn from(value: TogglerSize) -> Self {
        (value as u32).into()
    }
}

pub fn radio_button<'a, M: Clone + 'a>(selected: bool, on_select: M) -> Radio<'a, M> {
    // Standalone on/off radio: a dummy `true` group value, filled when `selected`.
    radio("", true, selected.then_some(true), move |_| {
        on_select.clone()
    })
    .spacing(0)
}

pub fn labelled_radio<'a, M: Clone + 'a>(
    label: impl Display,
    selected: bool,
    on_select: M,
) -> Row<'a, M> {
    row![radio_button(selected, on_select), new::caption(label)]
        .spacing(LABEL_SPACING)
        .align_y(Alignment::Center)
}

pub fn labelled_checkbox<'a, M: Clone + 'a>(
    label: impl Into<Element<'a, M>>,
    checked: bool,
    on_toggle: impl Fn(bool) -> M + 'a,
) -> Row<'a, M> {
    let control: Element<'a, M> = checkbox_button(checked, on_toggle).into();
    let label: Element<'a, M> = label.into();
    row![control, label]
        .spacing(LABEL_SPACING)
        .align_y(Alignment::Center)
}

pub fn checkbox_button<'a, M: Clone + 'a>(
    checked: bool,
    on_toggle: impl Fn(bool) -> M + 'a,
) -> CheckBox<'a, M> {
    checkbox(checked).size(CHECKBOX_SIZE).on_toggle(on_toggle)
}

pub fn toggler_button<'a, M: Clone + 'a>(
    checked: bool,
    size: TogglerSize,
    on_toggle: impl Fn(bool) -> M + 'a,
) -> Toggler<'a, M> {
    Toggler::new(checked)
        .on_toggle(on_toggle)
        .size(size)
        .style(theme::toggler::primary)
}

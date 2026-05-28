use std::fmt::Display;

use iced::{widget::row, Alignment};

use crate::{
    component::{button::btn_copy, text::new},
    theme,
    widget::Element,
};

pub fn copyable_address<'a, M: Clone + 'a>(address: impl Display, clipboard: M) -> Element<'a, M> {
    let address = new::caption(address).style(theme::text::card_secondary);
    let cpy = btn_copy(Some(clipboard));
    row![address, cpy]
        .align_y(Alignment::Center)
        .spacing(12)
        .wrap()
        .into()
}

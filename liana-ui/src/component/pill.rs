use iced::{widget::tooltip, Length};

use crate::{component::text, theme, widget::*};

const PILL_PADDING: u16 = 10;

pub fn pill<'a, T: 'a>(label: &'a str, tooltip: &'a str) -> Container<'a, T> {
    Container::new({
        tooltip::Tooltip::new(
            Container::new(text::p2_regular(label))
                .padding(PILL_PADDING)
                .center_x(Length::Shrink)
                .style(theme::pill::simple),
            Container::new(text::p1_regular(tooltip))
                .padding(PILL_PADDING)
                .style(theme::card::simple),
            tooltip::Position::Top,
        )
    })
}

pub fn recovery<'a, T: 'a>() -> Container<'a, T> {
    pill("  Recovery  ", "This transaction is using a recovery path")
}

pub fn unconfirmed<'a, T: 'a>() -> Container<'a, T> {
    pill(
        "  Unconfirmed  ",
        "Do not treat this as a payment until it is confirmed",
    )
}

pub fn batch<'a, T: 'a>() -> Container<'a, T> {
    pill("  Batch  ", "This transaction contains multiple payments")
}

pub fn deprecated<'a, T: 'a>() -> Container<'a, T> {
    pill(
        "  Deprecated  ",
        "This transaction cannot be included in the blockchain anymore.",
    )
}

pub fn spent<'a, T: 'a>() -> Container<'a, T> {
    pill(
        "  Spent  ",
        "The transaction was included in the blockchain.",
    )
}

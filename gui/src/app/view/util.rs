use liana::miniscript::bitcoin::Amount;

use liana_ui::{color, component::text::*, util::Collection, widget::*};

pub fn amount<'a, T: 'a>(a: &Amount) -> impl Into<Element<'a, T>> {
    amount_with_size(a, P1_SIZE)
}

pub fn amount_with_size<'a, T: 'a>(a: &Amount, size: u16) -> impl Into<Element<'a, T>> {
    let spacing = if size > P1_SIZE { 10 } else { 5 };
    let sats = format!("{:.8}", a.to_btc());
    assert!(sats.len() >= 9);
    let row = Row::new()
        .spacing(spacing)
        .push(split_digits(sats[0..sats.len() - 6].to_string(), size).into())
        .push(if a.to_sat() < 1_000_000 {
            split_digits(sats[sats.len() - 6..sats.len() - 3].to_string(), size).into()
        } else {
            Row::new()
                .push(
                    text(sats[sats.len() - 6..sats.len() - 3].to_string())
                        .bold()
                        .size(size),
                )
                .into()
        })
        .push(if a.to_sat() < 1000 {
            split_digits(sats[sats.len() - 3..sats.len()].to_string(), size).into()
        } else {
            Row::new()
                .push(
                    text(sats[sats.len() - 3..sats.len()].to_string())
                        .bold()
                        .size(size),
                )
                .into()
        });

    Row::with_children(vec![row.into(), text("BTC").size(size).into()])
        .spacing(spacing)
        .align_items(iced::Alignment::Center)
}

fn split_digits<'a, T: 'a>(mut s: String, size: u16) -> impl Into<Element<'a, T>> {
    let prefixes = vec!["0.00", "0.0", "0.", "000", "00", "0"];
    for prefix in prefixes {
        if s.starts_with(prefix) {
            let right = s.split_off(prefix.len());
            return Row::new()
                .push(text(s).size(size).style(color::GREY_2))
                .push_maybe(if right.is_empty() {
                    None
                } else {
                    Some(text(right).bold().size(size))
                });
        }
    }
    Row::new().push(text(s).bold().size(size))
}

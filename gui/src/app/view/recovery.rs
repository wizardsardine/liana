use iced::{widget::Space, Alignment, Length};

use liana::miniscript::bitcoin::Amount;

use liana_ui::{
    component::{button, form, text::*},
    icon,
    util::Collection,
    widget::*,
};

use crate::app::view::message::{CreateSpendMessage, Message};

#[allow(clippy::too_many_arguments)]
pub fn recovery<'a>(
    locked_coins: &(usize, Amount),
    recoverable_coins: &(usize, Amount),
    feerate: &form::Value<String>,
    address: &'a form::Value<String>,
) -> Element<'a, Message> {
    Column::new()
        .push(Space::with_height(Length::Units(100)))
        .push(
            Row::new()
                .push(Container::new(
                    icon::recovery_icon().width(Length::Units(100)).size(50),
                ))
                .push(text("Recover the funds").size(50).bold())
                .align_items(Alignment::Center)
                .spacing(1),
        )
        .push(
            Container::new(Row::new().push(text(format!(
                "{} ({} coins) will be spendable through the recovery path in the next block",
                recoverable_coins.1, recoverable_coins.0
            ))))
            .center_x(),
        )
        .push_maybe(if *locked_coins != (0, Amount::from_sat(0)) {
            Some(
                Container::new(Row::new().push(text(format!(
                    "{} ({} coins) are not yet spendable through the recovery path",
                    locked_coins.1, locked_coins.0
                ))))
                .center_x(),
            )
        } else {
            None
        })
        .push(Space::with_height(Length::Units(20)))
        .push(
            Column::new()
                .push(text("Enter destination address and feerate:").bold())
                .push(
                    Container::new(
                        form::Form::new("Address", address, move |msg| {
                            Message::CreateSpend(CreateSpendMessage::RecipientEdited(
                                0, "address", msg,
                            ))
                        })
                        .warning("Invalid Bitcoin address")
                        .size(20)
                        .padding(10),
                    )
                    .width(Length::Units(250)),
                )
                .push(
                    Container::new(
                        form::Form::new("Feerate (sat/vbyte)", feerate, move |msg| {
                            Message::CreateSpend(CreateSpendMessage::FeerateEdited(msg))
                        })
                        .warning("Invalid feerate")
                        .size(20)
                        .padding(10),
                    )
                    .width(Length::Units(250)),
                )
                .push(
                    if feerate.valid
                        && !feerate.value.is_empty()
                        && address.valid
                        && !address.value.is_empty()
                        && recoverable_coins.0 != 0
                    {
                        button::primary(None, "Next")
                            .on_press(Message::Next)
                            .width(Length::Units(200))
                    } else {
                        button::primary(None, "Next").width(Length::Units(200))
                    },
                )
                .spacing(20)
                .align_items(Alignment::Center),
        )
        .align_items(Alignment::Center)
        .spacing(20)
        .into()
}

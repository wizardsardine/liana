use iced::{
    pure::{column, container, row, widget, Element},
    Alignment, Length,
};

use crate::{
    app::view::{message::*, modal},
    ui::{
        component::{
            button, form,
            text::{text, Text},
        },
        icon,
        util::Collection,
    },
};

pub fn choose_recipients_view<'a>(
    recipients: Vec<Element<'a, Message>>,
    is_valid: bool,
) -> Element<'a, Message> {
    modal(
        false,
        None,
        column()
            .push(text("Choose recipients").bold().size(50))
            .push(
                column()
                    .push(widget::Column::with_children(recipients).spacing(10))
                    .push(
                        button::transparent(Some(icon::plus_icon()), "Add recipient")
                            .on_press(Message::CreateSpend(CreateSpendMessage::AddRecipient)),
                    )
                    .max_width(1000)
                    .spacing(10),
            )
            .push_maybe(if is_valid {
                Some(
                    button::primary(None, "Next")
                        .on_press(Message::Next)
                        .width(Length::Units(100)),
                )
            } else {
                None
            })
            .spacing(20)
            .align_items(Alignment::Center),
    )
}

pub fn recipient_view<'a>(
    index: usize,
    address: &form::Value<String>,
    amount: &form::Value<String>,
) -> Element<'a, CreateSpendMessage> {
    row()
        .push(
            form::Form::new("Address", address, move |msg| {
                CreateSpendMessage::RecipientEdited(index, "address", msg)
            })
            .warning("Please enter correct bitcoin address")
            .size(20)
            .padding(10),
        )
        .push(
            container(
                form::Form::new("Amount", amount, move |msg| {
                    CreateSpendMessage::RecipientEdited(index, "amount", msg)
                })
                .warning("Please enter correct amount")
                .size(20)
                .padding(10),
            )
            .width(Length::Units(250)),
        )
        .spacing(5)
        .push(
            button::transparent(Some(icon::trash_icon()), "")
                .on_press(CreateSpendMessage::DeleteRecipient(index))
                .width(Length::Shrink),
        )
        .align_items(Alignment::Center)
        .width(Length::Fill)
        .into()
}

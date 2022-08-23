use iced::pure::{column, container, pick_list, row, scrollable, Element};
use iced::{Alignment, Length};

use crate::ui::{
    component::{
        button, form,
        text::{text, Text},
    },
    util::Collection,
};

use crate::installer::message::{self, Message};

const NETWORKS: [bitcoin::Network; 4] = [
    bitcoin::Network::Bitcoin,
    bitcoin::Network::Testnet,
    bitcoin::Network::Signet,
    bitcoin::Network::Regtest,
];

pub fn welcome(network: &bitcoin::Network) -> Element<Message> {
    container(container(
        column()
            .push(container(
                pick_list(&NETWORKS[..], Some(*network), message::Message::Network).padding(10),
            ))
            .push(
                button::primary(None, "Install")
                    .on_press(Message::Next)
                    .width(Length::Units(200)),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(100)
            .spacing(50)
            .align_items(Alignment::Center),
    ))
    .center_y()
    .center_x()
    .height(Length::Fill)
    .width(Length::Fill)
    .into()
}

pub fn define_descriptor<'a>(
    imported_descriptor: &form::Value<String>,
    user_xpub: &form::Value<String>,
    heir_xpub: &form::Value<String>,
    sequence: &form::Value<String>,
    error: Option<&String>,
) -> Element<'a, Message> {
    let col_descriptor = column()
        .push(text("Descriptor:").bold())
        .push(
            form::Form::new("Descriptor", imported_descriptor, |msg| {
                Message::DefineDescriptor(message::DefineDescriptor::ImportDescriptor(msg))
            })
            .warning("Please enter correct descriptor")
            .size(20)
            .padding(10),
        )
        .spacing(10);

    let col_user_xpub = column()
        .push(text("Your xpub:").bold())
        .push(
            form::Form::new("Xpub", user_xpub, |msg| {
                Message::DefineDescriptor(message::DefineDescriptor::UserXpubEdited(msg))
            })
            .warning("Please enter correct xpub")
            .size(20)
            .padding(10),
        )
        .spacing(10);

    let col_heir_xpub = column()
        .push(text("Heir xpub:").bold())
        .push(
            form::Form::new("Xpub", heir_xpub, |msg| {
                Message::DefineDescriptor(message::DefineDescriptor::HeirXpubEdited(msg))
            })
            .warning("Please enter correct xpub")
            .size(20)
            .padding(10),
        )
        .spacing(10);

    let col_sequence = column()
        .push(text("Number of block").bold())
        .push(
            form::Form::new("Number of block", sequence, |msg| {
                Message::DefineDescriptor(message::DefineDescriptor::SequenceEdited(msg))
            })
            .warning("Please enter correct block number")
            .size(20)
            .padding(10),
        )
        .spacing(10);

    layout(
        column()
            .push(text("Create the descriptor").bold().size(50))
            .push(
                column()
                    .push(col_user_xpub)
                    .push(
                        row()
                            .push(col_sequence.width(Length::FillPortion(1)))
                            .push(col_heir_xpub.width(Length::FillPortion(4)))
                            .spacing(20),
                    )
                    .spacing(20),
            )
            .push(text("or import it").bold().size(25))
            .push(col_descriptor)
            .push(
                if !imported_descriptor.value.is_empty()
                    && (!user_xpub.value.is_empty()
                        || !heir_xpub.value.is_empty()
                        || !sequence.value.is_empty())
                {
                    button::primary(None, "Next").width(Length::Units(200))
                } else {
                    button::primary(None, "Next")
                        .width(Length::Units(200))
                        .on_press(Message::Next)
                },
            )
            .push_maybe(error.map(|e| text(e).size(15)))
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(100)
            .spacing(50)
            .align_items(Alignment::Center),
    )
}

pub fn define_bitcoin<'a>(
    address: &form::Value<String>,
    cookie_path: &form::Value<String>,
) -> Element<'a, Message> {
    let col_address = column()
        .push(text("Address:").bold())
        .push(
            form::Form::new("Address", address, |msg| {
                Message::DefineBitcoind(message::DefineBitcoind::AddressEdited(msg))
            })
            .warning("Please enter correct address")
            .size(20)
            .padding(10),
        )
        .spacing(10);

    let col_cookie = column()
        .push(text("Cookie path:").bold())
        .push(
            form::Form::new("Cookie path", cookie_path, |msg| {
                Message::DefineBitcoind(message::DefineBitcoind::CookiePathEdited(msg))
            })
            .warning("Please enter correct path")
            .size(20)
            .padding(10),
        )
        .spacing(10);

    layout(
        column()
            .push(
                text("Set up connection to the Bitcoin full node")
                    .bold()
                    .size(50),
            )
            .push(col_address)
            .push(col_cookie)
            .push(
                button::primary(None, "Next")
                    .on_press(Message::Next)
                    .width(Length::Units(200)),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(100)
            .spacing(50)
            .align_items(Alignment::Center),
    )
}

pub fn install<'a>(
    generating: bool,
    config_path: Option<&std::path::PathBuf>,
    warning: Option<&String>,
) -> Element<'a, Message> {
    let mut col = column()
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(100)
        .spacing(50)
        .align_items(Alignment::Center);

    if let Some(error) = warning {
        col = col.push(text(error));
    }

    if generating {
        col = col.push(button::primary(None, "Installing ...").width(Length::Units(200)))
    } else if let Some(path) = config_path {
        col = col.push(
            container(
                column()
                    .push(container(text("Installed !")))
                    .push(container(
                        button::primary(None, "Start")
                            .on_press(Message::Exit(path.clone()))
                            .width(Length::Units(200)),
                    ))
                    .align_items(Alignment::Center)
                    .spacing(20),
            )
            .padding(50)
            .width(Length::Fill)
            .center_x(),
        );
    } else {
        col = col.push(
            button::primary(None, "Finalize installation")
                .on_press(Message::Install)
                .width(Length::Units(200)),
        );
    }

    layout(col)
}

fn layout<'a>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    container(scrollable(
        column()
            .push(
                container(button::transparent(None, "< Previous").on_press(Message::Previous))
                    .padding(5),
            )
            .push(container(content).width(Length::Fill).center_x()),
    ))
    .center_x()
    .height(Length::Fill)
    .width(Length::Fill)
    .into()
}

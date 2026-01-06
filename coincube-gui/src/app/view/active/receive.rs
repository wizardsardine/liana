use coincube_ui::{
    component::{button, text::*},
    icon, theme,
    widget::*,
};
use iced::{
    widget::{qr_code, text::Wrapping, Column, Container, QRCode, Row},
    Alignment, Length,
};

use crate::app::view::{ActiveReceiveMessage, ReceiveMethod};

pub fn active_receive_view<'a>(
    receive_method: &'a ReceiveMethod,
    address: Option<&'a String>,
    qr_data: Option<&'a qr_code::Data>,
    loading: bool,
    amount_input: &'a str,
    description_input: &'a str,
) -> Element<'a, ActiveReceiveMessage> {
    let mut content = Column::new()
        .spacing(40)
        .width(Length::Fill)
        .align_x(Alignment::Center)
        .padding(40);

    content = content.push(method_toggle(receive_method));

    // Show input fields only for Lightning
    if *receive_method == ReceiveMethod::Lightning {
        content = content.push(input_fields(amount_input, description_input));
    } else {
        // For on-chain, only show generate button if no address is displayed
        if address.is_none() && !loading {
            content = content.push(generate_button());
        }
    }

    if loading {
        content = content.push(
            Container::new(
                Column::new()
                    .spacing(20)
                    .align_x(Alignment::Center)
                    .push(text("Generating address...").size(18)),
            )
            .width(Length::Fill)
            .center_x(Length::Fill),
        );
    } else if let (Some(addr), Some(qr)) = (address, qr_data) {
        // Lightning invoices contain more data, so use smaller cell size
        let cell_size = if *receive_method == ReceiveMethod::Lightning {
            4
        } else {
            8
        };

        // Clean on-chain addresses for display (but keep original for QR code)
        let display_addr = if *receive_method == ReceiveMethod::OnChain {
            let cleaned = addr.strip_prefix("bitcoin:").unwrap_or(addr);
            cleaned.split('?').next().unwrap_or(cleaned)
        } else {
            addr
        };

        content = content.push(
            Column::new()
                .spacing(30)
                .align_x(Alignment::Center)
                .push(
                    Container::new(QRCode::<theme::Theme>::new(qr).cell_size(cell_size))
                        .padding(30)
                        .style(theme::card::simple),
                )
                .push(
                    Container::new(
                        text(display_addr)
                            .size(12)
                            .style(theme::text::secondary)
                            .wrapping(Wrapping::Glyph),
                    )
                    .width(Length::Fill)
                    .max_width(600)
                    .padding(10)
                    .center_x(Length::Fill),
                )
                .push(action_buttons(receive_method)),
        );

        // Add generate new address button for on-chain
        if *receive_method == ReceiveMethod::OnChain {
            content = content.push(
                Container::new(
                    button::secondary(None, "Generate New Address")
                        .on_press(ActiveReceiveMessage::GenerateAddress)
                        .width(Length::Fixed(200.0))
                        .padding(10),
                )
                .width(Length::Fill)
                .center_x(Length::Fill)
                .padding(10),
            );
        }
    }

    content.into()
}

fn method_toggle(current_method: &ReceiveMethod) -> Element<ActiveReceiveMessage> {
    let lightning_button = Container::new(
        button::transparent(Some(icon::lightning_icon()), "Lightning")
            .on_press(ActiveReceiveMessage::ToggleMethod(ReceiveMethod::Lightning))
            .width(Length::Fixed(200.0))
            .padding(15),
    )
    .style(if *current_method == ReceiveMethod::Lightning {
        theme::container::border_orange
    } else {
        theme::container::border_grey
    });

    let onchain_button = Container::new(
        button::transparent(Some(icon::bitcoin_icon()), "On-chain")
            .on_press(ActiveReceiveMessage::ToggleMethod(ReceiveMethod::OnChain))
            .width(Length::Fixed(200.0))
            .padding(15),
    )
    .style(if *current_method == ReceiveMethod::OnChain {
        theme::container::border_orange
    } else {
        theme::container::border_grey
    });

    Row::new()
        .spacing(20)
        .push(lightning_button)
        .push(onchain_button)
        .into()
}

fn input_fields<'a>(
    amount_input: &'a str,
    description_input: &'a str,
) -> Element<'a, ActiveReceiveMessage> {
    let amount_field = Column::new()
        .spacing(8)
        .push(text("Amount (sats)").size(14).style(theme::text::secondary))
        .push(
            TextInput::new("Optional", amount_input)
                .on_input(ActiveReceiveMessage::AmountInput)
                .padding(12)
                .width(Length::Fill),
        );

    let description_field = Column::new()
        .spacing(8)
        .push(text("Description").size(14).style(theme::text::secondary))
        .push(
            TextInput::new("Optional", description_input)
                .on_input(ActiveReceiveMessage::DescriptionInput)
                .padding(12)
                .width(Length::Fill),
        );

    let generate_btn = button::primary(None, "Generate Invoice")
        .on_press(ActiveReceiveMessage::GenerateAddress)
        .width(Length::Fill)
        .padding(15);

    Container::new(
        Column::new()
            .spacing(15)
            .max_width(500)
            .push(amount_field)
            .push(description_field)
            .push(generate_btn),
    )
    .width(Length::Fill)
    .center_x(Length::Fill)
    .into()
}

fn generate_button<'a>() -> Element<'a, ActiveReceiveMessage> {
    Container::new(
        button::primary(None, "Generate Address")
            .on_press(ActiveReceiveMessage::GenerateAddress)
            .width(Length::Fixed(200.0))
            .padding(15),
    )
    .width(Length::Fill)
    .center_x(Length::Fill)
    .into()
}

fn action_buttons(_receive_method: &ReceiveMethod) -> Element<ActiveReceiveMessage> {
    let copy_button = button::primary(Some(icon::clipboard_icon()), "Copy")
        .on_press(ActiveReceiveMessage::Copy)
        .width(Length::Fixed(150.0))
        .padding(15);

    Row::new().spacing(15).push(copy_button).into()
}

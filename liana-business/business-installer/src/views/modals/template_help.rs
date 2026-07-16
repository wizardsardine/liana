use crate::state::{views::modals::TemplateHelpModalState, Msg};
use iced::{
    alignment::Vertical,
    widget::{column, row, Space},
    Padding,
};
use liana_ui::{
    component::{button, modal::ModalWidth, text},
    icon,
    spacing::{HSpacing, VSpacing},
    theme,
    widget::*,
};

pub fn template_help_modal_view(_modal_state: &TemplateHelpModalState) -> Element<'_, Msg> {
    let title = row![
        icon::tooltip_icon().size(20).style(theme::text::warning),
        text::new::b1_bold("Can't approve this template?")
    ]
    .spacing(HSpacing::M)
    .align_y(Vertical::Center);

    let message = text::new::caption(
        "To request edits, reach out to Wizardsardine directly: they can make the changes and send it back for approval.",
    )
    .style(theme::text::secondary);

    let footer = row![
        Space::fill_width(),
        button::btn_email_wizardsardine(Some(Msg::TemplateHelpEmailWs)),
        Space::fill_width()
    ]
    .spacing(HSpacing::M);

    let close = button::btn_modal_close(Some(Msg::TemplateHelpCloseModal));
    let header = row![title, Space::fill_width(), close].align_y(Vertical::Center);
    let body = column![header, message, footer]
        .spacing(VSpacing::M)
        .padding(20)
        .width(ModalWidth::M as u32);

    Container::new(body)
        .padding(Padding {
            top: 0.0,
            right: 20.0,
            bottom: 20.0,
            left: 20.0,
        })
        .style(theme::card::modal)
        .into()
}

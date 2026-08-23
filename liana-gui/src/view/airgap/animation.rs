use iced::widget::column;
use liana_ui::{
    component::{modal::ModalWidth, qr},
    spacing::VSpacing,
    widget::Element,
};

use crate::airgap::animation::AnimatedQr;

pub fn animation<'a, M: Clone + 'a>(
    animation: &'a AnimatedQr,
    enlarged: bool,
    modal_width: ModalWidth,
    on_press: M,
) -> Element<'a, M> {
    let frame = qr::frame(animation.frame(), enlarged, on_press);
    if enlarged {
        return frame;
    }
    column![frame, qr::progress(animation.progress(), modal_width)]
        .spacing(VSpacing::M)
        .into()
}

use iced::{widget::column, Alignment, Length};
use liana_ui::{
    component::{modal::ModalWidth, qr, text::new::caption},
    spacing::VSpacing,
    widget::Element,
};

use crate::{airgap::camera::CameraScanner, t};

const FRAME_HEIGHT: u32 = 300;

pub fn camera<'a, M: 'a>(scanner: &'a CameraScanner, modal_width: ModalWidth) -> Element<'a, M> {
    let frame = scanner
        .frame
        .clone()
        .map(|handle| qr::camera_frame(handle, FRAME_HEIGHT));
    let looking = scanner
        .progress
        .is_none()
        .then(|| caption(t!("airgap-looking-for-qr")));
    let progress = scanner
        .progress
        .as_ref()
        .and_then(|progress| qr::progress(progress, modal_width));
    column![frame, looking, progress]
        .spacing(VSpacing::M)
        .align_x(Alignment::Center)
        .width(Length::Fill)
        .into()
}

pub mod animation;
pub mod camera;

use bip377::{
    backend::{rust_bitcoin::RustBitcoin, Backend},
    request,
};
use iced::{
    widget::{column, mouse_area, row, Space},
    Alignment, Length,
};
use liana_ui::{
    component::{
        address, button,
        modal::{modal_view, ModalWidth},
        notification, pick_list, qr,
        text::new::{b5_medium, caption},
    },
    icon,
    spacing::{HSpacing, VSpacing},
    theme,
    widget::{Container, Element, SpaceExt},
};

use crate::{
    airgap::display::QrSettings,
    app::state::airgap::{AirgapModal, Message, Phase},
    t,
};

const MODAL_WIDTH: ModalWidth = ModalWidth::XL;
const CAMERA_REFUSED_WIDTH: ModalWidth = ModalWidth::M;
const SETTINGS_ICON_SIZE: u32 = 12;

pub fn modal<M: Clone + 'static>(
    modal: &AirgapModal,
    to_message: impl Fn(Message) -> M + Clone + 'static,
) -> Element<'_, M> {
    if modal.enlarged && modal.phase == Phase::ShowRequest {
        return mouse_area(animation::animation(
            &modal.animation,
            true,
            MODAL_WIDTH,
            to_message(Message::ToggleEnlarged),
        ))
        .on_press(to_message(Message::Cancel))
        .into();
    }

    if modal.phase == Phase::CameraRefused {
        let allow = if cfg!(target_os = "windows") {
            t!("airgap-camera-allow-windows")
        } else if cfg!(target_os = "macos") {
            t!("airgap-camera-allow-macos")
        } else {
            t!("airgap-camera-allow-linux")
        };
        let actions = row![
            button::btn_back_to_request(Some(to_message(Message::Back))),
            Space::fill_width(),
            button::btn_retry(Some(to_message(Message::RetryCamera))),
        ]
        .spacing(HSpacing::M)
        .align_y(Alignment::Center);
        let content = column![
            caption(t!("airgap-camera-refused")),
            caption(allow),
            actions
        ]
        .spacing(VSpacing::L)
        .width(Length::Fill);
        return modal_view(
            Some(t!("airgap-camera-refused-title")),
            None,
            Some(to_message(Message::Cancel)),
            CAMERA_REFUSED_WIDTH,
            content,
        );
    }

    let warning = modal.error.as_ref().map(|error| {
        notification::warning(
            format!("{}: {error}", t!("airgap-error-title")),
            error.to_string(),
        )
        .width(Length::Fill)
    });
    let body = match modal.phase {
        Phase::ShowRequest => request_view(modal, to_message.clone()),
        Phase::Scanning => scanning_view(modal, to_message.clone()),
        Phase::CameraRefused | Phase::Done => Space::fill_width().into(),
    };
    let content = column![warning, body]
        .spacing(VSpacing::L)
        .width(Length::Fill);
    modal_view(
        Some(&modal.title),
        None,
        Some(to_message(Message::Cancel)),
        MODAL_WIDTH,
        content,
    )
}

fn request_view<M: Clone + 'static>(
    modal: &AirgapModal,
    to_message: impl Fn(Message) -> M + Clone + 'static,
) -> Element<'_, M> {
    let request = animation::animation(
        &modal.animation,
        false,
        MODAL_WIDTH,
        to_message(Message::ToggleEnlarged),
    );

    let address = match modal.exchange.request() {
        request::Body::VerifyAddress(request::VerifyAddress {
            address: Some(address),
            ..
        }) => Some(
            Container::new(address::address(RustBitcoin::to_ll_address(address)))
                .center_x(Length::Fill),
        ),
        _ => None,
    };

    let display = modal.animation.settings();
    let rank = QrSettings::DENSITIES
        .iter()
        .position(|density| *density == display.density)
        .unwrap_or_default();
    let density = qr::setting_slider(
        t!("qr-density"),
        0..=(QrSettings::DENSITIES.len() - 1) as u8,
        rank as u8,
        {
            let to_message = to_message.clone();
            move |rank| to_message(Message::Density(QrSettings::DENSITIES[usize::from(rank)]))
        },
        to_message(Message::SaveSettings),
    );
    let frame_rate = qr::setting_slider(
        t!("qr-frame-rate"),
        frame_rate_step(*QrSettings::FRAME_RATES.start())
            ..=frame_rate_step(*QrSettings::FRAME_RATES.end()),
        frame_rate_step(display.frame_rate),
        {
            let to_message = to_message.clone();
            move |step| {
                to_message(Message::FrameRate(
                    f32::from(step) * QrSettings::FRAME_RATE_STEP,
                ))
            }
        },
        to_message(Message::SaveSettings),
    );
    let brightness = qr::setting_slider(
        t!("qr-brightness", percent = display.brightness.percent()),
        qr::Brightness::MIN..=qr::Brightness::MAX,
        display.brightness.percent(),
        {
            let to_message = to_message.clone();
            move |percent| to_message(Message::Brightness(qr::Brightness::new(percent)))
        },
        to_message(Message::SaveSettings),
    );
    let chevron = if modal.settings_expanded {
        icon::collapsed_icon()
    } else {
        icon::collapse_icon()
    };
    let settings_toggle = button::btn_optional_section(
        row![
            caption(t!("qr-settings")),
            chevron
                .size(SETTINGS_ICON_SIZE)
                .style(theme::text::secondary),
        ]
        .spacing(HSpacing::S)
        .align_y(Alignment::Center),
        to_message(Message::ToggleSettings),
    );
    let listed = modal.cameras.as_deref().unwrap_or_default();
    let cameras = (listed.len() > 1).then(|| {
        let names: Vec<String> = listed.iter().map(|camera| camera.name.clone()).collect();
        let selected = modal.camera().ok().map(|camera| camera.name.clone());
        let picker = pick_list::pick_list(names, selected, {
            let to_message = to_message.clone();
            move |name| to_message(Message::SelectCamera(name))
        })
        .padding(pick_list::PICK_LIST_PADDING);
        column![caption(t!("qr-camera")), picker]
            .spacing(VSpacing::SM)
            .align_x(Alignment::Center)
    });
    let settings = modal
        .settings_expanded
        .then(|| column![density, frame_rate, brightness, cameras].spacing(VSpacing::M));

    column![
        b5_medium(t!("airgap-show-request")),
        request,
        address,
        button::btn_scan_response(Some(to_message(Message::Scan))),
        settings_toggle,
        settings,
    ]
    .spacing(VSpacing::M)
    .align_x(Alignment::Center)
    .width(Length::Fill)
    .into()
}

fn scanning_view<M: Clone + 'static>(
    modal: &AirgapModal,
    to_message: impl Fn(Message) -> M + Clone + 'static,
) -> Element<'_, M> {
    let scanner = modal
        .scanner
        .as_ref()
        .map(|scanner| camera::camera(scanner, MODAL_WIDTH));
    column![
        b5_medium(t!("airgap-point-camera")),
        scanner,
        button::btn_back_to_request(Some(to_message(Message::Back))),
    ]
    .spacing(VSpacing::M)
    .align_x(Alignment::Center)
    .width(Length::Fill)
    .into()
}

fn frame_rate_step(rate: f32) -> u8 {
    (rate / QrSettings::FRAME_RATE_STEP).round() as u8
}

use iced::{
    widget::{column, row},
    Alignment, Length,
};
use liana_i18n::t;
use std::fmt::Display;

use crate::{
    component::{
        badge, button,
        text::{caption, text, Text},
    },
    icon,
    widget::{Button, Column, Container, Element, Row},
};

const PADDING: u16 = 10;
const SPACING: u32 = 20;

fn breadcrumb_btn<M: Clone + 'static>(label: impl Display, msg: Option<M>) -> Button<'static, M> {
    button::breadcrumb(None, label).on_press_maybe(msg)
}

pub fn header<M: Clone + 'static>(
    setting_msg: Option<M>,
    section_title: Option<String>,
    msg: Option<M>,
) -> Element<'static, M> {
    let setting_btn = breadcrumb_btn(t!("menu-settings"), setting_msg);
    let section_btn = section_title.map(|t| breadcrumb_btn(t, msg));

    if let Some(s_btn) = section_btn {
        row![setting_btn, icon::chevron_right().size(30), s_btn]
    } else {
        row![setting_btn]
    }
    .align_y(Alignment::Center)
    .into()
}

pub enum SectionKind {
    General,
    Node,
    Backend,
    Wallet,
    ImportExport,
    About,
}

impl SectionKind {
    pub fn title(&self) -> String {
        match self {
            SectionKind::General => t!("settings-section-general"),
            SectionKind::Node => t!("settings-section-node"),
            SectionKind::Backend => t!("settings-section-backend"),
            SectionKind::Wallet => t!("settings-section-wallet"),
            SectionKind::ImportExport => t!("settings-section-import-export"),
            SectionKind::About => t!("settings-section-about"),
        }
    }

    pub fn icon<M>(&self) -> Container<'static, M> {
        match self {
            SectionKind::General => badge::setting(),
            SectionKind::Node | SectionKind::Backend => badge::bitcoin(),
            SectionKind::Wallet | SectionKind::ImportExport => badge::wallet(),
            SectionKind::About => badge::tooltip(),
        }
    }
}

pub enum ImportExportKind {
    ImportWallet,
    ExportWallet,
    ExportLabels,
    ExportTransactions,
    ExportDescriptor,
    ExportEncryptedDescriptor,
}

impl ImportExportKind {
    pub fn title_descr(&self) -> (String, String) {
        match self {
            ImportExportKind::ImportWallet => (
                t!("settings-import-wallet"),
                t!("settings-import-wallet-description"),
            ),
            ImportExportKind::ExportWallet => (
                t!("settings-export-wallet"),
                t!("settings-export-wallet-description"),
            ),
            ImportExportKind::ExportLabels => (
                t!("settings-export-labels"),
                t!("settings-export-labels-description"),
            ),

            ImportExportKind::ExportTransactions => (
                t!("settings-export-transactions"),
                t!("settings-export-transactions-description"),
            ),
            ImportExportKind::ExportDescriptor => (
                t!("settings-export-descriptor"),
                t!("settings-export-descriptor-description"),
            ),
            ImportExportKind::ExportEncryptedDescriptor => (
                t!("settings-export-encrypted-descriptor"),
                t!("settings-export-encrypted-descriptor-description"),
            ),
        }
    }

    pub fn badge<M>(&self) -> Container<'static, M> {
        match self {
            ImportExportKind::ImportWallet => badge::restore(),
            _ => badge::backup(),
        }
    }
}

pub fn content_box<'a, M>(content: Row<'a, M>) -> Row<'a, M> {
    content
        .padding(PADDING)
        .spacing(SPACING)
        .align_y(Alignment::Center)
        .width(Length::Fill)
}

pub fn settings_section<M: Clone + 'static>(kind: SectionKind, msg: M) -> Element<'static, M> {
    let content = content_box(row![kind.icon(), text(kind.title()).bold()]);
    button::clickable_section(content, Some(msg)).into()
}

pub fn export_section<M: Clone + 'static>(kind: ImportExportKind, msg: M) -> Element<'static, M> {
    let (title, description) = kind.title_descr();
    let texts = column![text(title).bold(), caption(description)];
    let content = content_box(row![kind.badge(), texts,]);
    button::clickable_section(content, Some(msg)).into()
}

pub fn section_list<M: 'static + Clone>(children: Vec<Element<'static, M>>) -> Element<'static, M> {
    let header = header(None, None, None);
    let mut header = vec![header];
    header.extend(children);

    Column::from_vec(header)
        .spacing(20)
        .width(Length::Fill)
        .into()
}

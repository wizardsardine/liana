use std::fmt::Display;

use iced::{
    widget::{column, row},
    Alignment, Length,
};

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
    section_title: Option<impl Into<String>>,
    msg: Option<M>,
) -> Element<'static, M> {
    let setting_btn = breadcrumb_btn("Settings", setting_msg);
    let section_btn = section_title.map(|t| breadcrumb_btn(t.into(), msg));

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
            SectionKind::General => "General".to_string(),
            SectionKind::Node => "Node".to_string(),
            SectionKind::Backend => "Backend".to_string(),
            SectionKind::Wallet => "Wallet".to_string(),
            SectionKind::ImportExport => "ImportExport".to_string(),
            SectionKind::About => "About".to_string(),
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
                "Import wallet".to_string(),
                "Upload a backup file to update wallet info.".to_string(),
            ),
            ImportExportKind::ExportWallet => (
                "Export wallet".to_string(),
                "File (not encrypted) with wallet info useful to sync labels and data on other devices.".to_string(),
            ),
            ImportExportKind::ExportLabels => (
                "BIP 329 labels".to_string(),
                "BIP 329 label export, compatible with other wallets.".to_string(),
            ),

            ImportExportKind::ExportTransactions => (
                "Transactions table".to_string(),
                ".CSV file of past transactions, for accounting purposes.".to_string(),
            ),
            ImportExportKind::ExportDescriptor => (
                "Descriptor only - plain-text".to_string(),
                "Plain-text (not encrypted) descriptor file only, to use with other wallets.".to_string(),
            ),
            ImportExportKind::ExportEncryptedDescriptor => (
                "Encrypted descriptor".to_string(),
                ".bed file, can be decrypted with one of your signing devices or xpubs.".to_string(),
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
    let header = header(None, None::<String>, None);
    let mut header = vec![header];
    header.extend(children);

    Column::from_vec(header)
        .spacing(20)
        .width(Length::Fill)
        .into()
}

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

fn breadcrumb_btn<M: Clone + 'static>(label: &'static str, msg: Option<M>) -> Button<'static, M> {
    button::breadcrumb(None, label).on_press_maybe(msg)
}

pub fn header<M: Clone + 'static>(
    setting_msg: Option<M>,
    section_title: Option<&'static str>,
    msg: Option<M>,
) -> Element<'static, M> {
    let setting_btn = breadcrumb_btn("Settings", setting_msg);
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
    Payjoin,
    ImportExport,
    About,
}

impl SectionKind {
    pub fn title(&self) -> &'static str {
        match self {
            SectionKind::General => "General",
            SectionKind::Node => "Node",
            SectionKind::Backend => "Backend",
            SectionKind::Wallet => "Wallet",
            SectionKind::Payjoin => "Payjoin",
            SectionKind::ImportExport => "ImportExport",
            SectionKind::About => "About",
        }
    }

    pub fn icon<M>(&self) -> Container<'static, M> {
        match self {
            SectionKind::General => badge::setting(),
            SectionKind::Node | SectionKind::Backend => badge::bitcoin(),
            SectionKind::Wallet | SectionKind::ImportExport => badge::wallet(),
            SectionKind::Payjoin => badge::payjoin_symbol(),
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
    pub fn title_descr(&self) -> (&'static str, &'static str) {
        match self {
            ImportExportKind::ImportWallet => (
                "Import wallet",
                "Upload a backup file to update wallet info.",
            ),
            ImportExportKind::ExportWallet => (
                "Export wallet",
                "File (not encrypted) with wallet info useful to sync labels and data on other devices."
            ),
            ImportExportKind::ExportLabels => (
                "BIP 329 labels",
                "Bip 329 label export, compatible with other wallets."
            ),

            ImportExportKind::ExportTransactions => (
                "Transactions table",
                ".CSV file of past transactions, for accounting purposes."
            ),
            ImportExportKind::ExportDescriptor => (
                "Descriptor only - plain-text",
                "Plain-text (not encrypted) descriptor file only, to use with other wallets."
            ),
            ImportExportKind::ExportEncryptedDescriptor => (
                "Encrypted descriptor",
                ".bed file, can be decrypted with one of your signing devices or xpubs."
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

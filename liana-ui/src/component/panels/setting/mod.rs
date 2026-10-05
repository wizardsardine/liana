use std::fmt::Display;

use iced::{
    widget::{column, row, Space},
    Alignment, Length,
};
use liana_i18n::t;

use crate::{
    component::{
        badge::{self, Tile},
        button::{self, EntryWidth},
        card, list, separation,
        text::new,
    },
    icon,
    spacing::{HSpacing, VSpacing},
    theme,
    widget::{Button, Column, Container, Element, Row, SpaceExt},
};

fn breadcrumb_btn<M: Clone + 'static>(label: impl Display, msg: Option<M>) -> Button<'static, M> {
    button::breadcrumb(label).on_press_maybe(msg)
}

pub fn header<M: Clone + 'static>(
    setting_msg: Option<M>,
    section_title: Option<impl Into<String>>,
    msg: Option<M>,
) -> Element<'static, M> {
    let setting_btn = breadcrumb_btn(t!("menu-settings"), setting_msg);
    let section_btn = section_title.map(|t| breadcrumb_btn(t.into(), msg));

    if let Some(s_btn) = section_btn {
        row![setting_btn, icon::chevron_right().size(30), s_btn]
    } else {
        row![setting_btn]
    }
    .spacing(HSpacing::M)
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
            SectionKind::Wallet => t!("common-wallet"),
            SectionKind::ImportExport => t!("settings-section-import-export"),
            SectionKind::About => t!("settings-section-about"),
        }
    }

    pub fn tile(&self) -> Tile {
        match self {
            SectionKind::General => Tile::Setting,
            SectionKind::Node | SectionKind::Backend => Tile::Bitcoin,
            SectionKind::Wallet | SectionKind::ImportExport => Tile::Wallet,
            SectionKind::About => Tile::About,
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

    pub fn tile(&self) -> Tile {
        match self {
            ImportExportKind::ImportWallet => Tile::Import,
            _ => Tile::Backup,
        }
    }
}

pub fn settings_section<M: Clone + 'static>(kind: SectionKind, msg: M) -> Element<'static, M> {
    list::entry_section(
        kind.tile(),
        kind.title(),
        None::<String>,
        EntryWidth::Fill,
        Some(msg),
    )
}

pub fn export_section<M: Clone + 'static>(kind: ImportExportKind, msg: M) -> Element<'static, M> {
    let (title, description) = kind.title_descr();
    list::entry_section(
        kind.tile(),
        title,
        Some(description),
        EntryWidth::Fill,
        Some(msg),
    )
}

pub fn section_list<M: 'static + Clone>(children: Vec<Element<'static, M>>) -> Element<'static, M> {
    let header = header(None, None::<String>, None);
    let mut header = vec![header];
    header.extend(children);

    Column::from_vec(header)
        .spacing(VSpacing::L)
        .width(Length::Fill)
        .into()
}

pub fn setting_row<'a, M: 'a>(
    label: impl Display,
    info: Option<Element<'a, M>>,
    control: impl Into<Element<'a, M>>,
) -> Row<'a, M> {
    row![
        new::b4_medium(label),
        info,
        Space::fill_width(),
        control.into()
    ]
    .spacing(HSpacing::M)
    .align_y(Alignment::Center)
}

pub fn version_card<'a, M: 'a>(versions: Vec<String>) -> Container<'a, M> {
    let title = row![
        badge::tile(Tile::About),
        new::h3_semi(t!("settings-version"))
    ]
    .spacing(HSpacing::L)
    .align_y(Alignment::Center)
    .width(Length::Fill);
    let versions = Column::with_children(
        versions
            .into_iter()
            .map(|version| new::caption(version).style(theme::text::secondary).into()),
    );
    card::simple(column![
        title,
        separation().width(Length::Fill),
        Space::with_height(VSpacing::S),
        row![Space::fill_width(), versions]
    ])
}

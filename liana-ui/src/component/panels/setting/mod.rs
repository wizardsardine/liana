use std::fmt::Display;

use iced::{
    widget::{column, opaque, responsive, row, Space},
    Alignment, Length,
};
use liana_i18n::t;

use crate::{
    component::{
        badge::{self, Tile},
        button::{self, BtnWidth, EntryWidth},
        card,
        checkbox::{derivation_paths_checkbox, derivation_paths_checkbox_width},
        list, scrollable, separation,
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

/// An export section with an element aligned right, such as a toggler. A click on that
/// element does not trigger the section.
pub fn export_section_with_footer<M: Clone + 'static>(
    kind: ImportExportKind,
    msg: M,
    footer: impl Into<Element<'static, M>>,
) -> Element<'static, M> {
    let (title, description) = kind.title_descr();
    list::entry_section_with_footer(
        kind.tile(),
        title,
        Some(description),
        Some(opaque(footer)),
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

type OnToggle<M> = fn(bool) -> M;

pub fn descriptor_card<'a, M: Clone + 'a>(
    descriptor: String,
    copy: Option<M>,
    privacy_mode: Option<(bool, OnToggle<M>)>,
    backup: M,
    register: M,
) -> Container<'a, M> {
    let title = new::h3_semi(t!("settings-wallet-descriptor"));
    let descriptor_s = scrollable::horizontal_thin(new::caption(descriptor)).width(Length::Fill);
    let btn_copy = copy.map(|msg| button::btn_copy(Some(msg)));
    let descriptor_row = row![descriptor_s, btn_copy]
        .spacing(HSpacing::M)
        .align_y(Alignment::Center)
        .width(Length::Fill);
    // too narrow for one line: the register button goes under the backup one
    let btn_row = responsive(move |size| {
        let checkbox_width = if privacy_mode.is_some() {
            derivation_paths_checkbox_width() + HSpacing::XL.pixels() as f32
        } else {
            0.0
        };
        let one_line =
            f32::from(BtnWidth::XXL as u16) * 2.0 + HSpacing::M.pixels() as f32 + checkbox_width;
        let paths = privacy_mode.map(|(privacy_mode, on_toggle)| {
            Element::from(derivation_paths_checkbox(privacy_mode, on_toggle))
        });
        let backup = row![paths, button::btn_backup_encrypt_descriptor(backup.clone())]
            .spacing(HSpacing::XL)
            .align_y(Alignment::Center);
        let register = button::btn_register_on_device(register.clone());
        if size.width >= one_line {
            row![Space::fill_width(), backup, register]
                .spacing(HSpacing::M)
                .align_y(Alignment::Center)
                .into()
        } else {
            column![
                row![Space::fill_width(), backup],
                row![Space::fill_width(), register]
            ]
            .spacing(VSpacing::S)
            .into()
        }
    })
    .height(Length::Shrink);
    card::simple(
        column![title, descriptor_row, btn_row]
            .spacing(VSpacing::S)
            .width(Length::Fill),
    )
    .width(Length::Fill)
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

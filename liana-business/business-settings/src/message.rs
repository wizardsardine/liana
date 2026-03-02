//! Message types for business settings UI.

use liana_gui::{app::view::Close, export::ImportExportMessage};

/// Settings section for navigation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Section {
    #[default]
    Wallet,
    About,
}

/// Message type for business settings UI.
#[derive(Debug, Clone)]
pub enum Msg {
    /// Navigate back to settings home.
    Home,

    /// Navigate to a section.
    SelectSection(Section),

    /// Register wallet on selected device.
    RegisterWallet,

    /// Copy descriptor to clipboard.
    CopyDescriptor,

    /// Export encrypted descriptor.
    ExportEncryptedDescriptor,

    /// Handle export progress/result.
    Export(ImportExportMessage),
}

impl From<ImportExportMessage> for Msg {
    fn from(msg: ImportExportMessage) -> Self {
        Msg::Export(msg)
    }
}

impl Close for Msg {
    fn close() -> Self {
        Msg::Export(ImportExportMessage::Close)
    }
}

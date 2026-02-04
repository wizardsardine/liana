//! Message types for business settings UI.

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
}

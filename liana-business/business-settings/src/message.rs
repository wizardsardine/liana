//! Message types for business settings UI.

/// Settings section for navigation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Section {
    #[default]
    General,
    Wallet,
    About,
}

/// Message type for business settings UI.
#[derive(Debug, Clone)]
pub enum Msg {
    /// Navigate to a section.
    SelectSection(Section),

    /// Toggle fiat price display.
    EnableFiat(bool),

    /// Register wallet on selected device.
    RegisterWallet,
}

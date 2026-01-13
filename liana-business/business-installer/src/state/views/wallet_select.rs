/// State for the wallet selection view
#[derive(Debug, Clone)]
pub struct WalletSelectState {
    /// Whether to hide finalized wallets (WSManager only)
    pub hide_finalized: bool,
    /// Search filter text for wallet names
    pub search_filter: String,
}

impl Default for WalletSelectState {
    fn default() -> Self {
        Self {
            hide_finalized: true, // Selected by default
            search_filter: String::new(),
        }
    }
}

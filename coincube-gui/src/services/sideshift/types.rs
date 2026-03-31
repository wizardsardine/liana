use serde::{Deserialize, Serialize};

/// Supported external USDt networks (beyond native Liquid).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SideshiftNetwork {
    Liquid,
    Ethereum,
    Tron,
    Binance,
    Solana,
}

impl SideshiftNetwork {
    /// Human-readable display name shown in the UI (includes "USDt").
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Liquid => "Liquid USDt",
            Self::Ethereum => "Ethereum USDt",
            Self::Tron => "Tron USDt",
            Self::Binance => "Binance USDt",
            Self::Solana => "Solana USDt",
        }
    }

    /// Just the network name without "USDt".
    pub fn network_name(&self) -> &'static str {
        match self {
            Self::Liquid => "Liquid",
            Self::Ethereum => "Ethereum",
            Self::Tron => "Tron",
            Self::Binance => "Binance",
            Self::Solana => "Solana",
        }
    }

    /// Subtitle shown beneath external network options in the picker.
    pub fn swap_subtitle(&self) -> Option<&'static str> {
        match self {
            Self::Liquid => None,
            _ => Some("Swapped to Liquid USDt"),
        }
    }

    /// SideShift API `depositNetwork` / `settleNetwork` slug.
    pub fn network_slug(&self) -> &'static str {
        match self {
            Self::Liquid => "liquid",
            Self::Ethereum => "ethereum",
            Self::Tron => "tron",
            Self::Binance => "bsc",
            Self::Solana => "solana",
        }
    }

    /// Short standard label shown in the "Only for …" warning badge.
    pub fn standard_label(&self) -> &'static str {
        match self {
            Self::Liquid => "Liquid",
            Self::Ethereum => "ERC-20",
            Self::Tron => "TRC-20",
            Self::Binance => "BEP-20",
            Self::Solana => "SPL",
        }
    }

    /// Detect possible networks from a recipient address.
    /// Returns empty vec for unrecognised formats.
    pub fn detect_from_address(addr: &str) -> Vec<SideshiftNetwork> {
        let addr = addr.trim();
        if addr.is_empty() {
            return vec![];
        }

        // Liquid: confidential (VJL/VTp/VTq), blech32 (lq1/ex1), or unconfidential (Q/G/H)
        if addr.starts_with("VJL")
            || addr.starts_with("VTp")
            || addr.starts_with("VTq")
            || addr.starts_with("lq1")
            || addr.starts_with("ex1")
            || (addr.len() >= 34
                && (addr.starts_with('Q') || addr.starts_with('G') || addr.starts_with('H')))
        {
            return vec![Self::Liquid];
        }

        // Tron: starts with T, base58, 34 chars
        if addr.starts_with('T') && addr.len() == 34 && addr.chars().all(|c| c.is_alphanumeric()) {
            return vec![Self::Tron];
        }

        // EVM-compatible: 0x + 40 hex chars → ambiguous (Ethereum, Binance)
        if addr.starts_with("0x")
            && addr.len() == 42
            && addr[2..].chars().all(|c| c.is_ascii_hexdigit())
        {
            return vec![Self::Ethereum, Self::Binance];
        }

        // Solana: base58-encoded 32-byte public key (typically 43–44 chars).
        // Reject prefixes that belong to other networks (Bitcoin, Liquid).
        let solana_excluded_prefix = addr.starts_with('1')
            || addr.starts_with('3')
            || addr.starts_with("bc1")
            || addr.starts_with('Q')
            || addr.starts_with('G')
            || addr.starts_with('H');
        if !solana_excluded_prefix
            && (32..=44).contains(&addr.len())
            && addr.chars().all(|c| c.is_alphanumeric())
        {
            return vec![Self::Solana];
        }

        vec![]
    }

    /// Returns the external (non-Liquid) networks only.
    pub fn external() -> &'static [SideshiftNetwork] {
        &[Self::Ethereum, Self::Tron, Self::Binance, Self::Solana]
    }

    /// Returns all networks in display order.
    pub fn all() -> &'static [SideshiftNetwork] {
        &[
            Self::Liquid,
            Self::Ethereum,
            Self::Tron,
            Self::Binance,
            Self::Solana,
        ]
    }
}

// ---------------------------------------------------------------------------
// API request/response types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuoteRequest {
    pub deposit_coin: String,
    pub deposit_network: String,
    pub settle_coin: String,
    pub settle_network: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deposit_amount: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settle_amount: Option<String>,
    pub affiliate_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShiftQuote {
    pub id: String,
    pub deposit_coin: String,
    pub deposit_network: String,
    pub settle_coin: String,
    pub settle_network: String,
    pub deposit_amount: Option<String>,
    pub settle_amount: Option<String>,
    pub rate: Option<String>,
    pub affiliate_id: Option<String>,
    pub created_at: Option<String>,
    pub expires_at: Option<String>,
    pub min: Option<String>,
    pub max: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FixedShiftRequest {
    pub quote_id: String,
    pub settle_address: String,
    pub affiliate_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VariableShiftRequest {
    pub deposit_coin: String,
    pub deposit_network: String,
    pub settle_coin: String,
    pub settle_network: String,
    pub settle_address: String,
    pub affiliate_id: String,
}

/// Response returned by both fixed and variable shift creation endpoints.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShiftResponse {
    pub id: String,
    pub deposit_address: String,
    pub deposit_coin: Option<String>,
    pub deposit_network: Option<String>,
    pub settle_address: Option<String>,
    pub settle_coin: Option<String>,
    pub settle_network: Option<String>,
    pub deposit_min: Option<String>,
    pub deposit_max: Option<String>,
    pub deposit_amount: Option<String>,
    pub settle_amount: Option<String>,
    pub rate: Option<String>,
    pub expires_at: Option<String>,
    pub created_at: Option<String>,
    pub status: Option<String>,
    pub affiliate_fee_percent: Option<String>,
    pub network_fee_usd: Option<String>,
}

/// Status response from `GET /v2/shifts/{shiftId}`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShiftStatus {
    pub id: String,
    pub status: String,
    pub deposit_address: Option<String>,
    pub deposit_amount: Option<String>,
    pub settle_amount: Option<String>,
    pub expires_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShiftStatusKind {
    Waiting,
    Pending,
    Processing,
    Settling,
    Settled,
    Expired,
    Refunded,
    Error,
    Unknown(String),
}

impl From<&str> for ShiftStatusKind {
    fn from(s: &str) -> Self {
        match s {
            "waiting" => Self::Waiting,
            "pending" => Self::Pending,
            "processing" => Self::Processing,
            "settling" => Self::Settling,
            "settled" => Self::Settled,
            "expired" => Self::Expired,
            "refunded" => Self::Refunded,
            "error" => Self::Error,
            other => Self::Unknown(other.to_string()),
        }
    }
}

impl ShiftStatusKind {
    pub fn display(&self) -> &str {
        match self {
            Self::Waiting => "Waiting for deposit",
            Self::Pending => "Deposit detected",
            Self::Processing => "Processing",
            Self::Settling => "Settling",
            Self::Settled => "Settled",
            Self::Expired => "Expired",
            Self::Refunded => "Refunded",
            Self::Error => "Error",
            Self::Unknown(_) => "Unknown",
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Settled | Self::Expired | Self::Refunded | Self::Error
        )
    }
}

/// Backend response for `GET /api/v1/config/sideshift`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SideshiftConfig {
    pub affiliate_id: String,
}

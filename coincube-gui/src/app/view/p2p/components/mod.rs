pub mod order_card;
pub mod order_filters;
pub mod trade_card;

pub use order_card::{order_card, order_detail, OrderType, P2POrder, PricingMode};
pub use order_filters::{
    order_filter_sidebar, trade_status_filter, BuySellFilter, OrderFilterState, TradeFilter,
};
pub use trade_card::{trade_card, P2PTrade, TradeRole, TradeStatus};

/// Format a number with thousand separators (e.g. 1234567 → "1,234,567").
pub fn format_with_separators(n: u64) -> String {
    let s = n.to_string();
    let mut result = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(ch);
    }
    result.chars().rev().collect()
}

/// Format a premium percentage for display (e.g. `(+5%)`, `(-2%)`, `(0%)`).
pub fn format_premium(premium: Option<f64>) -> String {
    match premium {
        Some(p) if p > 0.0 => format!("(+{}%)", p),
        Some(p) if p < 0.0 => format!("({}%)", p),
        _ => "(0%)".to_string(),
    }
}

/// All fiat currency codes supported by Mostro P2P.
pub const FIAT_CURRENCIES: &[&str] = &[
    "AED", "ANG", "AOA", "ARS", "AUD", "AZN", "BDT", "BHD", "BOB", "BRL", "BWP", "BYN", "CAD",
    "CDF", "CHF", "CLP", "CNY", "COP", "CRC", "CUP", "CZK", "DKK", "DOP", "DZD", "EGP", "ETB",
    "EUR", "GBP", "GEL", "GHS", "GTQ", "HKD", "HNL", "HUF", "IDR", "ILS", "INR", "JMD", "JOD",
    "JPY", "KES", "KGS", "KRW", "KZT", "LBP", "LKR", "MAD", "MLC", "MWK", "MXN", "MYR", "NAD",
    "NGN", "NIO", "NOK", "NPR", "NZD", "PAB", "PEN", "PHP", "PKR", "PLN", "PYG", "QAR", "RON",
    "RSD", "RUB", "SAR", "SEK", "SGD", "THB", "TND", "TOP", "TRY", "TTD", "TWD", "TZS", "UAH",
    "UGX", "USD", "UYU", "UZS", "VES", "VND", "XAF", "XOF", "ZAR",
];

/// Payment methods per currency. Returns a static slice; falls back to a
/// default list for currencies not explicitly mapped.
pub fn payment_methods_for(currency: &str) -> &'static [&'static str] {
    match currency {
        "ARS" => &[
            "Mercado Pago",
            "MODO",
            "CVU",
            "Belo",
            "Lemon",
            "CBU",
            "Efectivo",
        ],
        "AUD" => &["PayID", "Alipay", "Cash deposit", "Revolut", "Cash"],
        "BOB" => &["QR", "Transferencia Bancaria", "Efectivo"],
        "BRL" => &["PIX", "TED", "PicPay", "Depósito", "Cash"],
        "CAD" => &[
            "Interac e-Transfer",
            "Any national bank",
            "Wise",
            "Revolut",
            "Cash",
        ],
        "CHF" => &["TWINT", "Cash"],
        "CLP" => &[
            "MACH",
            "Cuenta RUT",
            "Mercado Pago",
            "Transferencia bancaria",
            "Efectivo",
        ],
        "CRC" => &["Bank transfer (IBAN)", "SINPE Móvil", "Cash"],
        "COP" => &[
            "Nequi",
            "Daviplata",
            "PSE",
            "Llaves BRE-B",
            "Transferencia bancaria",
            "Efectivo",
        ],
        "CUP" => &[
            "Transfermovil",
            "EnZona",
            "Tarjeta Clásica",
            "Saldo móvil",
            "MiTransfer",
            "Efectivo",
        ],
        "EUR" => &[
            "Revolut",
            "HalCash",
            "Bank Transfer",
            "Wise",
            "SEPA instant",
            "Bizum",
            "Payoneer",
            "Cash",
        ],
        "GBP" => &[
            "Revolut",
            "Monzo",
            "Wise",
            "Any national bank",
            "Bank Transfer",
            "Cash",
        ],
        "JPY" => &["PayPay", "Bank Transfer", "Cash"],
        "MLC" => &["Transfermovil", "EnZona"],
        "MXN" => &["SPEI", "CoDi", "Retiro Cajero BBVA", "Efectivo"],
        "NGN" => &["Bank Transfer", "Cash"],
        "PEN" => &["Yape", "Plin", "QR Yape", "Transferencia bancaria", "Cash"],
        "PHP" => &["Any national bank", "GCash", "Cash"],
        "PYG" => &["SIPAP", "Transferencia bancaria", "Efectivo"],
        "USD" => &[
            "Cash App",
            "Venmo",
            "Zelle",
            "PayPal",
            "Wise",
            "Payoneer",
            "Strike",
            "Revolut",
            "N1CO",
            "Transfer365",
            "Cash",
        ],
        "VES" => &[
            "Pago Móvil",
            "Binance P2P",
            "Transferencia bancaria",
            "Efectivo",
        ],
        _ => &["Bank Transfer", "Cash in person"],
    }
}

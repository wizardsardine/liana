use super::currency::Currency;

use async_trait::async_trait;

use crate::services::http::NotSuccessResponseInfo;

/// A quote represents a finite, strictly positive amount of fiat per coin.
pub(super) fn is_usable_price(value: f64) -> bool {
    value.is_finite() && value > 0.0
}

#[derive(Debug, Clone)]
pub struct GetPriceResult {
    pub value: f64,
    pub updated_at: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct ListCurrenciesResult {
    pub currencies: Vec<Currency>,
}

#[derive(Debug, Clone)]
pub enum PriceApiError {
    RequestFailed(String),
    NotSuccessResponse(NotSuccessResponseInfo),
    CannotParseResponse(String),
    CannotParseData(String),
}

impl std::fmt::Display for PriceApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RequestFailed(e) => write!(f, "Request failed: {}", e),
            Self::NotSuccessResponse(info) => {
                // Nothing renders this today — `Error::FiatPrice` is mapped
                // to fixed copy by `crate::user_error` — but it is one
                // `Display` call away from a screen at all times, so it stays
                // safe to show: the envelope's own message if there is one,
                // never the raw body. Callers wanting the body for a log ask
                // for `raw_text()` directly.
                match info.message() {
                    Some(message) => {
                        write!(
                            f,
                            "Not success response ({}): {}",
                            info.status_code, message
                        )
                    }
                    None => write!(f, "Not success response ({})", info.status_code),
                }
            }
            Self::CannotParseResponse(e) => write!(f, "Cannot parse response: {}", e),
            Self::CannotParseData(e) => write!(f, "Cannot parse data: {}", e),
        }
    }
}

#[async_trait]
pub trait PriceApi {
    async fn get_price(&self, currency: Currency) -> Result<GetPriceResult, PriceApiError>;

    async fn list_currencies(&self) -> Result<ListCurrenciesResult, PriceApiError>;
}

#[cfg(test)]
mod tests {
    use super::{is_usable_price, PriceApiError};
    use crate::services::http::NotSuccessResponseInfo;

    #[test]
    fn usable_price_requires_a_finite_strictly_positive_value() {
        for value in [f64::NAN, f64::NEG_INFINITY, -1.0, -0.0, 0.0, f64::INFINITY] {
            assert!(!is_usable_price(value), "{}", value);
        }
        for value in [f64::MIN_POSITIVE, 0.125, f64::MAX] {
            assert!(is_usable_price(value), "{}", value);
        }
    }

    #[test]
    fn display_renders_actionable_error_messages() {
        assert_eq!(
            PriceApiError::RequestFailed("timeout".to_string()).to_string(),
            "Request failed: timeout"
        );
        assert_eq!(
            PriceApiError::CannotParseResponse("bad json".to_string()).to_string(),
            "Cannot parse response: bad json"
        );
        assert_eq!(
            PriceApiError::CannotParseData("price".to_string()).to_string(),
            "Cannot parse data: price"
        );
    }

    #[test]
    fn display_shows_the_envelope_message_and_never_the_raw_body() {
        let info = NotSuccessResponseInfo {
            status_code: 429,
            text: r#"{"success":false,"error":{"code":"rate_limited","message":"slow down"}}"#
                .to_string(),
        };
        assert_eq!(info.message().as_deref(), Some("slow down"));
        assert_eq!(info.code().as_deref(), Some("rate_limited"));

        let err = PriceApiError::NotSuccessResponse(info);
        assert_eq!(err.to_string(), "Not success response (429): slow down");
    }

    /// A body that is not our envelope must never be echoed into this
    /// `Display`, whether or not a screen happens to render it today.
    #[test]
    fn display_withholds_a_body_that_is_not_our_envelope() {
        let err = PriceApiError::NotSuccessResponse(NotSuccessResponseInfo {
            status_code: 502,
            text: "<html><title>502 Bad Gateway</title>nginx/1.24.0</html>".to_string(),
        });

        let shown = err.to_string();
        assert!(!shown.contains("nginx"), "raw body rendered: {}", shown);
        assert!(!shown.contains("<html>"), "raw body rendered: {}", shown);
        assert_eq!(shown, "Not success response (502)");
    }

    /// The transport arm is a `String`, so whatever is put in it is final.
    /// `client::get_data` scrubs the URL before it gets here; this pins that
    /// the endpoint cannot ride along.
    #[tokio::test]
    async fn a_transport_failure_carries_no_url() {
        let err =
            super::super::client::get_data_for_test("http://127.0.0.1:1/api/v1/prices/btc-usd")
                .await
                .expect_err("a closed port must fail");

        let shown = err.to_string();
        assert!(!shown.contains("127.0.0.1"), "host leaked: {}", shown);
        assert!(
            !shown.contains("prices/btc-usd"),
            "endpoint leaked: {}",
            shown
        );
    }
}

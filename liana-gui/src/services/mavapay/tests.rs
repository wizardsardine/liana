#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::mavapay::{Currency, MavapayClient, PaymentMethod, QuoteRequest};

    #[tokio::test]
    async fn test_mavapay_client_creation() {
        let client = MavapayClient::new("test_api_key".to_string());

        // Verify client is created with correct base URL for debug builds
        assert!(client.base_url.contains("staging.api.mavapay.co"));
    }

    #[test]
    fn test_currency_serialization() {
        use serde_json;

        let btc = Currency::BtcSat;
        let serialized = serde_json::to_string(&btc).unwrap();
        assert_eq!(serialized, "\"BTCSAT\"");

        let ngn = Currency::NgnKobo;
        let serialized = serde_json::to_string(&ngn).unwrap();
        assert_eq!(serialized, "\"NGNKOBO\"");
    }

    #[test]
    fn test_quote_request_creation() {
        let request = QuoteRequest {
            amount: 100000,
            source_currency: Currency::BtcSat,
            target_currency: Currency::NgnKobo,
            payment_method: PaymentMethod::Lightning,
        };

        assert_eq!(request.amount, 100000);
        assert_eq!(request.source_currency, Currency::BtcSat);
        assert_eq!(request.target_currency, Currency::NgnKobo);
        assert_eq!(request.payment_method, PaymentMethod::Lightning);
    }

    #[test]
    fn test_currency_symbol() {
        assert_eq!(Currency::BtcSat.symbol(), "sats");
        assert_eq!(Currency::NgnKobo.symbol(), "kobo");
        assert_eq!(Currency::ZarCent.symbol(), "cents");
        assert_eq!(Currency::KesCent.symbol(), "cents");
    }

    #[test]
    fn test_payment_method_as_str() {
        assert_eq!(PaymentMethod::Lightning.as_str(), "LIGHTNING");
        assert_eq!(PaymentMethod::BankTransfer.as_str(), "BANKTRANSFER");
        assert_eq!(PaymentMethod::NgnBankTransfer.as_str(), "NGNBANKTRANSFER");
    }

    #[test]
    fn test_price_parsing_actual_api_response() {
        use crate::services::mavapay::api::PriceResponse;
        use serde_json::json;

        // Test the actual API response format from the error message
        let api_response = json!({
            "btcPriceInUnitCurrency": "161804928.49992642",
            "currency": "NGN",
            "timestamp": "1759067919",
            "unitPricePerSat": {
                "amount": "1.6180492849992645",
                "currencyUnit": "NGNSAT"
            },
            "unitPricePerUsd": {
                "amount": "1475.346404",
                "currencyUnit": "NGNUSD"
            }
        });

        // Simulate the parsing logic from get_price method
        let price = if let Some(btc_price) = api_response
            .get("btcPriceInUnitCurrency")
            .and_then(|p| p.as_str())
        {
            btc_price.parse::<f64>().unwrap()
        } else if let Some(unit_price) = api_response
            .get("unitPricePerSat")
            .and_then(|p| p.get("amount"))
            .and_then(|a| a.as_str())
        {
            unit_price.parse::<f64>().unwrap()
        } else {
            panic!("Should be able to parse price from actual API response");
        };

        // Verify the price was parsed correctly
        assert_eq!(price, 161804928.49992642);

        // Test alternative parsing path (unitPricePerSat)
        let mut alt_response = api_response.clone();
        alt_response
            .as_object_mut()
            .unwrap()
            .remove("btcPriceInUnitCurrency");

        let alt_price = if let Some(btc_price) = alt_response
            .get("btcPriceInUnitCurrency")
            .and_then(|p| p.as_str())
        {
            btc_price.parse::<f64>().unwrap()
        } else if let Some(unit_price) = alt_response
            .get("unitPricePerSat")
            .and_then(|p| p.get("amount"))
            .and_then(|a| a.as_str())
        {
            unit_price.parse::<f64>().unwrap()
        } else {
            panic!("Should be able to parse price from unitPricePerSat");
        };

        assert_eq!(alt_price, 1.6180492849992645);
    }

    #[test]
    fn test_price_parsing_legacy_formats() {
        use serde_json::json;

        // Test legacy format with direct price field
        let legacy_response = json!({
            "price": 50000.0,
            "currency": "USD"
        });

        let price = if let Some(btc_price) = legacy_response
            .get("btcPriceInUnitCurrency")
            .and_then(|p| p.as_str())
        {
            btc_price.parse::<f64>().unwrap()
        } else if let Some(unit_price) = legacy_response
            .get("unitPricePerSat")
            .and_then(|p| p.get("amount"))
            .and_then(|a| a.as_str())
        {
            unit_price.parse::<f64>().unwrap()
        } else if let Some(p) = legacy_response.get("price").and_then(|p| p.as_f64()) {
            p
        } else {
            panic!("Should be able to parse price from legacy format");
        };

        assert_eq!(price, 50000.0);

        // Test nested data format
        let nested_response = json!({
            "data": {
                "btcPriceInUnitCurrency": "45000.123",
                "currency": "USD"
            }
        });

        let nested_price = if let Some(btc_price) = nested_response
            .get("btcPriceInUnitCurrency")
            .and_then(|p| p.as_str())
        {
            btc_price.parse::<f64>().unwrap()
        } else if let Some(data) = nested_response.get("data") {
            if let Some(btc_price) = data.get("btcPriceInUnitCurrency").and_then(|p| p.as_str()) {
                btc_price.parse::<f64>().unwrap()
            } else {
                panic!("Should be able to parse price from nested data");
            }
        } else {
            panic!("Should be able to parse price from nested format");
        };

        assert_eq!(nested_price, 45000.123);
    }
}

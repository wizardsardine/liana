// breez/client.rs
use breez_sdk_liquid::prelude as breez;
use std::sync::Arc;

pub struct BreezClient {
    sdk: Arc<breez::LiquidSdk>,
}

impl BreezClient {
    pub async fn connect(cfg: BreezConfig, mnemonic: &str) -> Result<Self, BreezError> {
        let request = breez::ConnectRequest {
            config: cfg.sdk_config(),
            mnemonic: Some(mnemonic.to_owned()),
            ..Default::default()
        };
        let sdk = breez::LiquidSdk::connect(request)
            .await
            .map_err(|e| BreezError::Connection(e.to_string()))?;
        Ok(Self { sdk: Arc::new(sdk) })
    }

    pub async fn info(&self) -> Result<breez::GetInfoResponse, BreezError> {
        self.sdk
            .get_info()
            .await
            .map_err(|e| BreezError::Sdk(e.to_string()))
    }

    pub async fn receive_invoice(
        &self,
        amount_sat: Option<u64>,
        description: Option<String>,
    ) -> Result<breez::ReceivePaymentResponse, BreezError> {
        let prepare = self
            .sdk
            .prepare_receive_payment(&breez::PrepareReceiveRequest {
                payment_method: breez::PaymentMethod::Bolt11Invoice,
                amount: amount_sat.map(|sat| breez::ReceiveAmount::Bitcoin {
                    payer_amount_sat: sat,
                }),
            })
            .await
            .map_err(|e| BreezError::Sdk(e.to_string()))?;

        self.sdk
            .receive_payment(&breez::ReceivePaymentRequest {
                prepare_response: prepare,
                description,
                ..Default::default()
            })
            .await
            .map_err(|e| BreezError::Sdk(e.to_string()))
    }

    pub async fn pay_invoice(
        &self,
        invoice: String,
        amount_sat: Option<u64>,
    ) -> Result<breez::SendPaymentResponse, BreezError> {
        let prepare = self
            .sdk
            .prepare_send_payment(&breez::PrepareSendRequest {
                destination: invoice,
                amount: amount_sat.map(|sat| breez::PayAmount::Bitcoin {
                    receiver_amount_sat: sat,
                }),
            })
            .await
            .map_err(|e| BreezError::Sdk(e.to_string()))?;

        self.sdk
            .send_payment(&breez::SendPaymentRequest {
                prepare_response: prepare,
                ..Default::default()
            })
            .await
            .map_err(|e| BreezError::Sdk(e.to_string()))
    }
}
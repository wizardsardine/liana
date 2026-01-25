pub mod api;

use crate::{
    app::view::buysell::meld,
    services::{coincube::CoincubeError, http::ResponseExt},
};

use iced::futures;
use reqwest::Method;

pub struct MeldClient<'client>(pub &'client super::coincube::CoincubeClient);

impl<'client> MeldClient<'client> {
    pub async fn get_supported_countries(
        &'client self,
    ) -> Result<Vec<api::MeldCountry>, CoincubeError> {
        let url = format!("{}/api/v1/meld/countries", self.0.base_url);
        let response = self
            .0
            .client
            .request(Method::GET, &url)
            .query(&[("accountFilter", "true"), ("cryptoCurrencies", "BTC")])
            .send()
            .await?;

        let response = response.check_success().await?;
        Ok(response.json().await?)
    }

    pub async fn get_fiat_purchase_limits<'a>(
        &'client self,
        currency_code: &'a str,
        country_code: &'a str,
    ) -> Result<Vec<api::CurrencyLimit>, CoincubeError> {
        let url = format!("{}/api/v1/meld/fiat-currency-buy-limits", self.0.base_url);
        let response = self
            .0
            .client
            .request(Method::GET, &url)
            .query(&[
                ("fiatCurrencies", currency_code),
                ("countries", country_code),
                ("accountFilter", "true"),
                ("cryptoCurrencies", "BTC"),
            ])
            .send()
            .await?;

        let response = response.check_success().await?;
        Ok(response.json().await?)
    }

    pub async fn get_crypto_sell_limits<'a>(
        &'client self,
        currency_code: &'a str,
        country_code: &'a str,
    ) -> Result<Vec<api::CurrencyLimit>, CoincubeError> {
        let url = format!("{}/api/v1/meld/crypto-sell-limits", self.0.base_url);
        let response = self
            .0
            .client
            .request(Method::GET, &url)
            .query(&[
                ("fiatCurrencies", currency_code),
                ("countries", country_code),
                ("accountFilter", "true"),
                ("cryptoCurrencies", "BTC"),
            ])
            .send()
            .await?;

        let response = response.check_success().await?;
        Ok(response.json().await?)
    }

    pub async fn get_quotes<'a>(
        &'client self,
        req: api::GetQuotesRequest<'a>,
    ) -> Result<api::GetQuoteResponse, CoincubeError> {
        let url = format!("{}/api/v1/meld/quote", self.0.base_url);
        let response = self
            .0
            .client
            .request(Method::POST, &url)
            .json(&req)
            .send()
            .await?;

        let response = response.check_success().await?;
        Ok(response.json().await?)
    }

    pub async fn create_session<'a>(
        &'client self,
        req: api::CreateSessionRequest<'a>,
    ) -> Result<api::CreateSessionResponse, CoincubeError> {
        let url = format!("{}/api/v1/meld/session/create", self.0.base_url);
        let response = self
            .0
            .client
            .request(Method::POST, &url)
            .json(&req)
            .send()
            .await?;

        let response = response.check_success().await?;
        Ok(response.json().await?)
    }

    pub fn transactions_subscription(
        token: String,
        retries: usize,
    ) -> iced::Subscription<crate::app::view::buysell::meld::MeldMessage> {
        iced::Subscription::run_with((token, retries), create_subscription)
    }
}

fn create_subscription(
    data: &(String, usize),
) -> impl iced::futures::Stream<Item = meld::MeldMessage> + 'static {
    use futures::SinkExt;
    use reqwest_sse::EventSource;

    #[cfg(debug_assertions)]
    let base_url = "https://dev-events.coincube.io";
    #[cfg(not(debug_assertions))]
    let base_url = env!("EVENTS_API_URL");

    let (token, retries) = data;

    let auth = format!("Bearer {}", token);
    let url = format!("{}/api/v1/meld/stream/transactions", base_url);

    // attempt to parse parameters
    let init = match reqwest::Url::parse(&url) {
        Ok(url) => match auth.parse() {
            Ok(a) => {
                let mut req = reqwest::Request::new(reqwest::Method::GET, url);

                req.headers_mut().append("Authorization", a);
                Some(req)
            }
            Err(err) => {
                log::error!("[MELD] Unable to start subscription, {}", err);
                None
            }
        },
        Err(err) => {
            log::error!("[MELD] Unable to start subscription, {:?}", err);
            None
        }
    };

    log::trace!(
        "[MELD] Starting subscription execution: Attempt #{}",
        retries
    );

    iced::stream::channel(
        8,
        |mut channel: futures::channel::mpsc::Sender<meld::MeldMessage>| async move {
            if let Some(request) = init {
                // send request
                match reqwest::Client::new().execute(request).await {
                    // query event source
                    Ok(res) => {
                        log::trace!("[MELD] EventSource pre-request was successful");

                        match res.events().await {
                            Ok(mut source) => loop {
                                // 30s heartbeat to ensure SSE is alive
                                let timeout =
                                    tokio::time::sleep(std::time::Duration::from_secs(60));
                                let event = futures::TryStreamExt::try_next(&mut source);

                                futures::pin_mut!(timeout);
                                futures::pin_mut!(event);

                                match futures::future::select(timeout, event).await {
                                    futures::future::Either::Left(_) => {
                                        let _ = channel
                                        .send(meld::MeldMessage::EventSourceDisconnected(
                                            "EventSource heartbeat failure, Client is probably offline".to_string(),
                                        ))
                                        .await;

                                        break;
                                    }
                                    futures::future::Either::Right((event, _)) => match event {
                                        Ok(Some(ev)) => {
                                            if ev.event_type == "transactionUpdate" {
                                                if let Err(err) = channel
                                                    .send(meld::MeldMessage::SseEvent(ev))
                                                    .await
                                                {
                                                    if err.is_disconnected() {
                                                        log::trace!("[MELD] Exiting subscription, Meld state was dropped");
                                                        break;
                                                    }
                                                };
                                            } else {
                                                log::trace!("[MELD] Got event from SSE: {:?}", ev)
                                            }
                                        }
                                        Ok(None) => {
                                            log::info!("[MELD] EventSource exiting safely");
                                            break;
                                        }
                                        Err(err) => {
                                            channel
                                                .send(meld::MeldMessage::EventSourceDisconnected(
                                                    err.to_string(),
                                                ))
                                                .await
                                                .unwrap();

                                            break;
                                        }
                                    },
                                };
                            },
                            Err(err) => channel
                                .send(meld::MeldMessage::SessionError(
                                    err.to_string(),
                                    "Unable to reach Coincube servers for SSE events",
                                ))
                                .await
                                .unwrap(),
                        }
                    }
                    Err(err) => channel
                        .send(meld::MeldMessage::SessionError(
                            err.to_string(),
                            "EventSource pre-request failed",
                        ))
                        .await
                        .unwrap(),
                };
            };
        },
    )
}

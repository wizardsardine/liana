pub mod api;

use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::Arc,
};

use async_trait::async_trait;
use chrono::Utc;
use liana::{
    commands::{CoinStatus, GetInfoDescriptors, LCSpendInfo, LabelItem},
    config::Config,
    descriptors::LianaDescriptor,
    miniscript::bitcoin::{address, psbt::Psbt, Address, Network, OutPoint, Txid},
};
use reqwest::{Error, IntoUrl, Method, RequestBuilder, Response};
use tokio::sync::RwLock;

use crate::{
    app::settings::{AuthConfig, Settings},
    daemon::{model::*, Daemon, DaemonBackend, DaemonError},
    hw::HardwareWalletConfig,
};

use self::api::{UTXOKind, DEFAULT_OUTPOINTS_LIMIT};

use super::auth::{self, AccessTokenResponse, AuthError};

impl From<Error> for DaemonError {
    fn from(value: Error) -> Self {
        DaemonError::Http(None, value.to_string())
    }
}

impl From<AuthError> for DaemonError {
    fn from(value: AuthError) -> Self {
        DaemonError::Http(value.http_status, value.error)
    }
}

fn request<U: IntoUrl>(
    http: &reqwest::Client,
    method: Method,
    url: U,
    access_token: &str,
) -> RequestBuilder {
    let req = http
        .request(method, url)
        .header("Authorization", format!("Bearer {}", access_token))
        .header("Content-Type", "application/json");
    tracing::debug!("Sending http request: {:?}", req);
    req
}

#[derive(Debug, Clone)]
pub struct BackendClient {
    pub auth: Arc<RwLock<auth::AccessTokenResponse>>,
    auth_client: auth::AuthClient,

    url: String,
    network: Network,
    http: reqwest::Client,

    user_id: String,
}

impl BackendClient {
    pub async fn connect(
        auth_client: auth::AuthClient,
        url: String,
        credentials: auth::AccessTokenResponse,
    ) -> Result<Self, DaemonError> {
        let http = reqwest::Client::new();
        let response = request(
            &http,
            Method::GET,
            &format!("{}/v1/me", url),
            &credentials.access_token,
        )
        .send()
        .await?;
        if !response.status().is_success() {
            return Err(DaemonError::NoAnswer);
        }
        let res: api::Claims = response.json().await?;
        let user_id = res.sub;

        Ok(Self {
            auth: Arc::new(RwLock::new(credentials)),
            auth_client,
            network: Network::Signet,
            url,
            user_id,
            http,
        })
    }

    pub fn user_email(&self) -> &str {
        &self.auth_client.email
    }

    pub async fn connect_first(self) -> Result<(BackendWalletClient, api::Wallet), DaemonError> {
        let wallets = self.list_wallets().await?;
        let first = wallets.first().cloned().ok_or(DaemonError::NoAnswer)?;
        Ok(self.connect_wallet(first))
    }

    pub fn connect_wallet(self, wallet: api::Wallet) -> (BackendWalletClient, api::Wallet) {
        (
            BackendWalletClient {
                inner: self,
                curve: secp256k1::Secp256k1::verification_only(),
                wallet_uuid: wallet.id.clone(),
                wallet_desc: wallet.descriptor.to_owned(),
            },
            wallet,
        )
    }

    async fn request<U: IntoUrl>(&self, method: Method, url: U) -> RequestBuilder {
        let access_token = &self.auth.read().await.access_token;
        request(&self.http, method, url, access_token)
    }

    pub async fn list_wallets(&self) -> Result<Vec<api::Wallet>, DaemonError> {
        let response = self
            .request(Method::GET, &format!("{}/v1/wallets", self.url))
            .await
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(DaemonError::Http(
                Some(response.status().into()),
                response.text().await?,
            ));
        }

        let list: api::ListWallets = response.json().await?;
        Ok(list.wallets)
    }

    pub async fn create_wallet(
        &self,
        name: &str,
        descriptor: &LianaDescriptor,
    ) -> Result<api::Wallet, DaemonError> {
        let response = self
            .request(Method::POST, &format!("{}/v1/wallets", self.url))
            .await
            .json(&api::payload::CreateWallet { name, descriptor })
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(DaemonError::Http(
                Some(response.status().into()),
                response.text().await?,
            ));
        }

        let wallet: api::Wallet = response.json().await?;
        Ok(wallet)
    }

    pub async fn update_wallet_metadata(
        &self,
        wallet_uuid: &str,
        fingerprint_aliases: &HashMap<Fingerprint, String>,
        hws: &[HardwareWalletConfig],
    ) -> Result<(), DaemonError> {
        let wallets = self.list_wallets().await?;
        let wallet = wallets
            .iter()
            .find(|w| w.id == wallet_uuid)
            .ok_or(DaemonError::Http(
                Some(404),
                "No wallet exists for this uuid".to_string(),
            ))?;
        let ledger_kinds = [
            async_hwi::DeviceKind::Ledger.to_string(),
            async_hwi::DeviceKind::LedgerSimulator.to_string(),
        ];
        for cfg in hws {
            if ledger_kinds.contains(&cfg.kind)
                && !wallet.metadata.ledger_hmacs.iter().any(|ledger_hmac| {
                    ledger_hmac.fingerprint == cfg.fingerprint && ledger_hmac.hmac == cfg.token
                })
            {
                let response: Response = self
                    .request(
                        Method::PATCH,
                        &format!("{}/v1/wallets/{}", self.url, wallet_uuid),
                    )
                    .await
                    .json(&api::payload::UpdateWallet {
                        ledger_hmac: Some(api::payload::UpdateLedgerHmac {
                            fingerprint: cfg.fingerprint.to_string(),
                            hmac: cfg.token.clone(),
                        }),
                        fingerprint_aliases: None,
                    })
                    .send()
                    .await?;

                if !response.status().is_success() {
                    return Err(DaemonError::Http(
                        Some(response.status().into()),
                        response.text().await?,
                    ));
                }
            }
        }

        if fingerprint_aliases.iter().any(|(fg, alias)| {
            !wallet
                .metadata
                .fingerprint_aliases
                .contains(&api::FingerprintAlias {
                    alias: alias.to_string(),
                    user_id: self.user_id.clone(),
                    fingerprint: *fg,
                })
        }) {
            let response: Response = self
                .request(
                    Method::PATCH,
                    &format!("{}/v1/wallets/{}", self.url, wallet_uuid),
                )
                .await
                .json(&api::payload::UpdateWallet {
                    ledger_hmac: None,
                    fingerprint_aliases: Some(
                        fingerprint_aliases
                            .iter()
                            .map(|(fg, alias)| api::payload::UpdateFingerprintAlias {
                                fingerprint: fg.to_string(),
                                alias: alias.to_string(),
                            })
                            .collect(),
                    ),
                })
                .send()
                .await?;

            if !response.status().is_success() {
                return Err(DaemonError::Http(
                    Some(response.status().into()),
                    response.text().await?,
                ));
            }
        }

        Ok(())
    }

    pub async fn get_wallet_invitation(
        &self,
        invitation_id: &str,
    ) -> Result<api::WalletInvitation, DaemonError> {
        let response = self
            .request(
                Method::GET,
                &format!("{}/v1/invitations/{}", self.url, invitation_id),
            )
            .await
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(DaemonError::Http(
                Some(response.status().into()),
                response.text().await?,
            ));
        }

        Ok(response.json().await?)
    }

    pub async fn accept_wallet_invitation(&self, invitation_id: &str) -> Result<(), DaemonError> {
        let response = self
            .request(
                Method::POST,
                &format!("{}/v1/invitations/{}/accept", self.url, invitation_id),
            )
            .await
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(DaemonError::Http(
                Some(response.status().into()),
                response.text().await?,
            ));
        }

        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct BackendWalletClient {
    inner: BackendClient,
    wallet_uuid: String,
    wallet_desc: LianaDescriptor,
    curve: secp256k1::Secp256k1<secp256k1::VerifyOnly>,
}

impl BackendWalletClient {
    pub fn inner_client(&self) -> &BackendClient {
        &self.inner
    }

    pub fn into_inner(self) -> BackendClient {
        self.inner
    }

    pub fn user_id(&self) -> &str {
        &self.inner.user_id
    }

    pub fn wallet_id(&self) -> String {
        self.wallet_uuid.clone()
    }

    pub fn user_email(&self) -> &str {
        self.inner.user_email()
    }

    async fn get_wallet(&self) -> Result<api::Wallet, DaemonError> {
        let list = self.inner.list_wallets().await?;
        let wallet = list
            .into_iter()
            .find(|w| w.id == self.wallet_uuid)
            .ok_or(DaemonError::Unexpected("No wallet".to_string()))?;
        Ok(wallet)
    }

    async fn list_psbts(&self, txids: &[Txid]) -> Result<api::ListPsbts, DaemonError> {
        let mut query = Vec::<(&str, String)>::new();
        if !txids.is_empty() {
            query.push((
                "txids",
                txids
                    .iter()
                    .map(|i| i.to_string())
                    .collect::<Vec<String>>()
                    .join(","),
            ))
        }
        let response: Response = self
            .inner
            .request(
                Method::GET,
                &format!("{}/v1/wallets/{}/psbts", self.inner.url, self.wallet_uuid),
            )
            .await
            .query(&query)
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(DaemonError::Http(
                Some(response.status().into()),
                response.text().await?,
            ));
        }

        response.json().await.map_err(DaemonError::from)
    }

    async fn list_txs_by_txids(
        &self,
        txids: &[Txid],
    ) -> Result<api::ListTransactions, DaemonError> {
        let mut query = Vec::<(&str, String)>::new();
        if !txids.is_empty() {
            query.push((
                "txids",
                txids
                    .iter()
                    .map(|i| i.to_string())
                    .collect::<Vec<String>>()
                    .join(","),
            ))
        } else {
            return Ok(api::ListTransactions {
                transactions: Vec::new(),
            });
        }
        let response: Response = self
            .inner
            .request(
                Method::GET,
                &format!(
                    "{}/v1/wallets/{}/transactions",
                    self.inner.url, self.wallet_uuid
                ),
            )
            .await
            .query(&query)
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(DaemonError::Http(
                Some(response.status().into()),
                response.text().await?,
            ));
        }

        response.json().await.map_err(DaemonError::from)
    }

    async fn list_wallet_txs(
        &self,
        before: Option<u32>,
        limit: Option<u64>,
    ) -> Result<api::ListTransactions, DaemonError> {
        let mut query = Vec::<(&str, String)>::new();
        if let Some(before) = before {
            query.push(("before", before.to_string()))
        }
        if let Some(limit) = limit {
            query.push(("limit", limit.to_string()))
        }
        let response: Response = self
            .inner
            .request(
                Method::GET,
                &format!(
                    "{}/v1/wallets/{}/transactions",
                    self.inner.url, self.wallet_uuid
                ),
            )
            .await
            .query(&query)
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(DaemonError::Http(
                Some(response.status().into()),
                response.text().await?,
            ));
        }

        response.json().await.map_err(DaemonError::from)
    }

    async fn list_wallet_coins(
        &self,
        statuses: &[CoinStatus],
        outpoints: &[OutPoint],
    ) -> Result<api::ListCoins, DaemonError> {
        let mut query = Vec::<(&'static str, String)>::new();
        if !statuses.is_empty() {
            query.push((
                "statuses",
                statuses
                    .iter()
                    .map(|s| s.to_arg())
                    .collect::<Vec<&str>>()
                    .join(","),
            ));
        }
        if !outpoints.is_empty() {
            query.push((
                "outpoints",
                outpoints
                    .iter()
                    .map(|o| o.to_string())
                    .collect::<Vec<String>>()
                    .join(","),
            ));
        }
        let response: Response = self
            .inner
            .request(
                Method::GET,
                &format!("{}/v1/wallets/{}/coins", self.inner.url, self.wallet_uuid),
            )
            .await
            .query(&query)
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(DaemonError::Http(
                Some(response.status().into()),
                response.text().await?,
            ));
        }

        let res: api::ListCoins = response.json().await?;
        Ok(res)
    }

    pub async fn auth(&self) -> AccessTokenResponse {
        self.inner.auth.read().await.clone()
    }
}

#[async_trait]
impl Daemon for BackendWalletClient {
    fn backend(&self) -> DaemonBackend {
        DaemonBackend::RemoteBackend
    }

    fn config(&self) -> Option<&Config> {
        None
    }

    /// refresh the token if close to expiration.
    async fn is_alive(&self, datadir: &Path, network: Network) -> Result<(), DaemonError> {
        let auth = self.auth().await;
        if auth.expires_at < Utc::now().timestamp() + 60 {
            match self.inner.auth.try_write() {
                Err(_) => {
                    // something is using the lock, we will try next time.
                    return Ok(());
                }
                Ok(mut old) => {
                    let new = self
                        .inner
                        .auth_client
                        .refresh_token(&auth.refresh_token)
                        .await?;

                    let mut settings = Settings::from_file(datadir.to_path_buf(), network)
                        .map_err(|e| {
                            DaemonError::Unexpected(format!(
                                "Cannot access to settings.json file: {}",
                                e
                            ))
                        })?;

                    if let Some(wallet_settings) = settings.wallets.iter_mut().find(|w| {
                        if let Some(auth) = &w.remote_backend_auth {
                            auth.wallet_id == self.wallet_uuid
                        } else {
                            false
                        }
                    }) {
                        wallet_settings.remote_backend_auth = Some(AuthConfig {
                            email: self.inner.auth_client.email.clone(),
                            wallet_id: self.wallet_id(),
                            refresh_token: new.refresh_token.clone(),
                        });
                    } else {
                        tracing::info!("Wallet id was not found in the settings");
                    }

                    settings
                        .to_file(datadir.to_path_buf(), network)
                        .map_err(|e| {
                            DaemonError::Unexpected(format!(
                                "Cannot access to settings.json file: {}",
                                e
                            ))
                        })?;

                    *old = new;
                    tracing::info!("Liana backend access was refreshed");
                }
            }
        }
        Ok(())
    }

    async fn stop(&self) -> Result<(), DaemonError> {
        Ok(())
    }

    async fn get_info(&self) -> Result<GetInfoResult, DaemonError> {
        let wallet = self.get_wallet().await?;

        Ok(GetInfoResult {
            network: self.inner.network,
            version: "".to_string(),
            block_height: wallet.tip_height.unwrap_or(0),
            descriptors: GetInfoDescriptors {
                main: wallet.descriptor.to_owned(),
            },
            sync: 1.0,
            rescan_progress: None,
            timestamp: wallet.created_at as u32,
        })
    }

    async fn get_new_address(&self) -> Result<GetAddressResult, DaemonError> {
        let response: Response = self
            .inner
            .request(
                Method::POST,
                &format!(
                    "{}/v1/wallets/{}/addresses",
                    self.inner.url, self.wallet_uuid
                ),
            )
            .await
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(DaemonError::Http(
                Some(response.status().into()),
                response.text().await?,
            ));
        }

        let res: api::Address = response.json().await?;
        Ok(GetAddressResult {
            address: res.address,
            derivation_index: res.derivation_index,
        })
    }

    /// Spent coins are not returned if statuses is empty, unless their outpoints are specified.
    async fn list_coins(
        &self,
        statuses: &[CoinStatus],
        outpoints: &[OutPoint],
    ) -> Result<ListCoinsResult, DaemonError> {
        let coins = if !outpoints.is_empty() {
            let mut coins = Vec::new();
            for chunk in outpoints.chunks(DEFAULT_OUTPOINTS_LIMIT) {
                coins.extend_from_slice(&self.list_wallet_coins(statuses, chunk).await?.coins);
            }
            coins
        } else {
            self.list_wallet_coins(statuses, outpoints).await?.coins
        };
        Ok(ListCoinsResult {
            coins: coins
                .into_iter()
                .map(|c| ListCoinsEntry {
                    address: c.address,
                    amount: c.amount,
                    derivation_index: c.derivation_index,
                    outpoint: c.outpoint,
                    block_height: c.block_height,
                    is_immature: c.is_immature,
                    is_change: c.is_change_address,
                    spend_info: c.spend_info.map(|info| LCSpendInfo {
                        txid: info.txid,
                        height: info.height,
                    }),
                })
                .collect(),
        })
    }

    async fn list_spend_txs(&self) -> Result<ListSpendResult, DaemonError> {
        let res = self.list_psbts(&[]).await?;
        Ok(ListSpendResult {
            spend_txs: res
                .psbts
                .into_iter()
                .map(|psbt| ListSpendEntry {
                    psbt: psbt.raw,
                    updated_at: Some(psbt.updated_at as u32),
                })
                .collect(),
        })
    }

    async fn list_confirmed_txs(
        &self,
        _start: u32,
        end: u32,
        limit: u64,
    ) -> Result<ListTransactionsResult, DaemonError> {
        let res = self.list_wallet_txs(Some(end), Some(limit)).await?;
        Ok(ListTransactionsResult {
            transactions: res
                .transactions
                .into_iter()
                .map(|tx| TransactionInfo {
                    tx: tx.raw,
                    height: tx.block_height,
                    time: tx.confirmed_at.map(|t| t as u32),
                })
                .collect(),
        })
    }

    async fn list_txs(&self, txids: &[Txid]) -> Result<ListTransactionsResult, DaemonError> {
        let mut transactions = Vec::new();
        if !txids.is_empty() {
            for chunk in txids.chunks(api::DEFAULT_LIMIT) {
                transactions.extend_from_slice(&self.list_txs_by_txids(chunk).await?.transactions);
            }
        }
        Ok(ListTransactionsResult {
            transactions: transactions
                .into_iter()
                .map(|tx| TransactionInfo {
                    tx: tx.raw,
                    height: tx.block_height,
                    time: tx.confirmed_at.map(|t| t as u32),
                })
                .collect(),
        })
    }

    async fn create_spend_tx(
        &self,
        coins_outpoints: &[OutPoint],
        destinations: &HashMap<Address<address::NetworkUnchecked>, u64>,
        feerate_vb: u64,
        change_address: Option<Address<address::NetworkUnchecked>>,
    ) -> Result<CreateSpendResult, DaemonError> {
        let mut recipients: Vec<api::payload::Recipient> = destinations
            .iter()
            .map(|(addr, amt)| api::payload::Recipient {
                amount: Some(*amt),
                address: addr.clone(),
                is_max: false,
            })
            .collect();
        if let Some(address) = change_address {
            recipients.push(api::payload::Recipient {
                amount: None,
                address,
                is_max: true,
            });
        }
        let response: Response = self
            .inner
            .request(
                Method::POST,
                &format!(
                    "{}/v1/wallets/{}/psbts/generate",
                    self.inner.url, self.wallet_uuid
                ),
            )
            .await
            .json(&api::payload::GeneratePsbt {
                save: false,
                feerate: feerate_vb,
                inputs: coins_outpoints,
                recipients,
            })
            .send()
            .await?;

        let res: api::DraftPsbtResult = response.json().await?;
        match res {
            api::DraftPsbtResult::Success(draft) => Ok(CreateSpendResult::Success {
                psbt: draft.raw,
                warnings: draft.warnings,
            }),
            api::DraftPsbtResult::InsufficientFunds(api::InsufficientFundsInfo { missing }) => {
                Ok(CreateSpendResult::InsufficientFunds { missing })
            }
        }
    }

    async fn rbf_psbt(
        &self,
        txid: &Txid,
        is_cancel: bool,
        feerate_vb: Option<u64>,
    ) -> Result<CreateSpendResult, DaemonError> {
        let response: Response = self
            .inner
            .request(
                Method::POST,
                &format!(
                    "{}/v1/wallets/{}/psbts/rbf",
                    self.inner.url, self.wallet_uuid
                ),
            )
            .await
            .json(&api::payload::GenerateRbfPsbt {
                txid: *txid,
                is_cancel,
                feerate: feerate_vb,
                save: false,
            })
            .send()
            .await?;

        let res: api::DraftPsbtResult = response.json().await?;
        match res {
            api::DraftPsbtResult::Success(draft) => Ok(CreateSpendResult::Success {
                psbt: draft.raw,
                warnings: draft.warnings,
            }),
            api::DraftPsbtResult::InsufficientFunds(api::InsufficientFundsInfo { missing }) => {
                Ok(CreateSpendResult::InsufficientFunds { missing })
            }
        }
    }

    async fn update_spend_tx(&self, psbt: &Psbt) -> Result<(), DaemonError> {
        let response: Response = self
            .inner
            .request(
                Method::POST,
                &format!("{}/v1/wallets/{}/psbts", self.inner.url, self.wallet_uuid),
            )
            .await
            .json(&api::payload::ImportPsbt {
                psbt: psbt.to_string(),
            })
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(DaemonError::Http(
                Some(response.status().into()),
                response.text().await?,
            ));
        }

        Ok(())
    }

    async fn delete_spend_tx(&self, txid: &Txid) -> Result<(), DaemonError> {
        let psbt = self
            .list_psbts(&[*txid])
            .await?
            .psbts
            .into_iter()
            .find(|tx| tx.txid == *txid)
            .ok_or(DaemonError::Http(
                Some(404),
                format!("psbt not found with txid: {}", txid),
            ))?;

        let response: Response = self
            .inner
            .request(
                Method::DELETE,
                &format!("{}/v1/psbts/{}", self.inner.url, psbt.uuid),
            )
            .await
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(DaemonError::Http(
                Some(response.status().into()),
                response.text().await?,
            ));
        }

        Ok(())
    }

    async fn broadcast_spend_tx(&self, txid: &Txid) -> Result<(), DaemonError> {
        let psbt = self
            .list_psbts(&[*txid])
            .await?
            .psbts
            .into_iter()
            .find(|tx| tx.txid == *txid)
            .ok_or(DaemonError::NoAnswer)?;

        let response: Response = self
            .inner
            .request(
                Method::POST,
                &format!("{}/v1/psbts/{}/broadcast", self.inner.url, psbt.uuid),
            )
            .await
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(DaemonError::Http(
                Some(response.status().into()),
                response.text().await?,
            ));
        }

        Ok(())
    }

    async fn start_rescan(&self, _t: u32) -> Result<(), DaemonError> {
        Err(DaemonError::NoAnswer)
    }

    async fn create_recovery(
        &self,
        address: Address<address::NetworkUnchecked>,
        feerate_vb: u64,
        sequence: Option<u16>,
    ) -> Result<Psbt, DaemonError> {
        let response: Response = self
            .inner
            .request(
                Method::POST,
                &format!(
                    "{}/v1/wallets/{}/psbts/recovery",
                    self.inner.url, self.wallet_uuid
                ),
            )
            .await
            .json(&api::payload::GenerateRecoveryPsbt {
                save: false,
                feerate: feerate_vb,
                timelock: sequence
                    .ok_or(DaemonError::Unexpected("Missing sequence".to_string()))?,
                address,
            })
            .send()
            .await?;

        let res: api::DraftPsbt = response.json().await?;
        Ok(res.raw)
    }

    async fn get_labels(
        &self,
        items: &HashSet<LabelItem>,
    ) -> Result<HashMap<String, String>, DaemonError> {
        if items.is_empty() {
            return Ok(HashMap::new());
        }
        let items: Vec<String> = items.iter().map(|item| item.to_string()).collect();
        let mut res = HashMap::new();
        for chunk in items.chunks(api::DEFAULT_LABEL_ITEMS_LIMIT) {
            let response: Response = self
                .inner
                .request(
                    Method::GET,
                    &format!("{}/v1/wallets/{}/labels", self.inner.url, self.wallet_uuid),
                )
                .await
                .query(&[("items", chunk.join(","))])
                .send()
                .await?;

            if !response.status().is_success() {
                return Err(DaemonError::Http(
                    Some(response.status().into()),
                    response.text().await?,
                ));
            }

            let wallet_labels: api::WalletLabels = response.json().await?;
            res.extend(wallet_labels.labels);
        }

        Ok(res)
    }

    async fn update_labels(
        &self,
        items: &HashMap<LabelItem, Option<String>>,
    ) -> Result<(), DaemonError> {
        let response: Response = self
            .inner
            .request(
                Method::POST,
                &format!("{}/v1/wallets/{}/labels", self.inner.url, self.wallet_uuid),
            )
            .await
            .json(&api::payload::Labels {
                labels: items
                    .iter()
                    .map(|(item, value)| api::payload::Label {
                        item: item.to_string(),
                        value: value.clone(),
                    })
                    .collect(),
            })
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(DaemonError::Http(
                Some(response.status().into()),
                response.text().await?,
            ));
        }

        Ok(())
    }

    async fn list_history_txs(
        &self,
        _start: u32,
        end: u32,
        limit: u64,
    ) -> Result<Vec<HistoryTransaction>, DaemonError> {
        let res = self
            .list_wallet_txs(Some(end), Some(limit))
            .await?
            .transactions
            .into_iter()
            .map(|tx| history_tx_from_api(tx, self.inner.network))
            .collect();
        Ok(res)
    }

    async fn get_history_txs(
        &self,
        txids: &[Txid],
    ) -> Result<Vec<HistoryTransaction>, DaemonError> {
        let mut transactions = Vec::new();
        if !txids.is_empty() {
            for chunk in txids.chunks(api::DEFAULT_LIMIT) {
                transactions.extend_from_slice(&self.list_txs_by_txids(chunk).await?.transactions);
            }
        }
        let res = transactions
            .into_iter()
            .map(|tx| history_tx_from_api(tx, self.inner.network))
            .collect();
        Ok(res)
    }
    async fn list_pending_txs(&self) -> Result<Vec<HistoryTransaction>, DaemonError> {
        let res = self
            .list_wallet_txs(None, None)
            .await?
            .transactions
            .into_iter()
            .filter_map(|tx| {
                if tx.block_height.is_none() {
                    Some(history_tx_from_api(tx, self.inner.network))
                } else {
                    None
                }
            })
            .collect();
        Ok(res)
    }

    async fn list_spend_transactions(
        &self,
        txids: Option<&[Txid]>,
    ) -> Result<Vec<SpendTx>, DaemonError> {
        let mut spend_txs: Vec<SpendTx> = if let Some(txids) = txids {
            let mut spend_txs = Vec::new();
            if !txids.is_empty() {
                for chunk in txids.chunks(api::DEFAULT_LIMIT) {
                    for tx in self.list_psbts(chunk).await?.psbts.into_iter().map(|tx| {
                        spend_tx_from_api(tx, &self.wallet_desc, &self.curve, self.inner.network)
                    }) {
                        spend_txs.push(tx);
                    }
                }
            }
            spend_txs
        } else {
            self.list_psbts(&[])
                .await?
                .psbts
                .into_iter()
                .map(|tx| spend_tx_from_api(tx, &self.wallet_desc, &self.curve, self.inner.network))
                .collect()
        };
        spend_txs.sort_by(|a, b| {
            if a.status == b.status {
                // last updated first
                b.updated_at.cmp(&a.updated_at)
            } else {
                // follows status enum order
                a.status.cmp(&b.status)
            }
        });
        Ok(spend_txs)
    }

    /// Implemented by LianaLite backend
    async fn update_wallet_metadata(
        &self,
        fingerprint_aliases: &HashMap<Fingerprint, String>,
        hws: &[HardwareWalletConfig],
    ) -> Result<(), DaemonError> {
        self.inner
            .update_wallet_metadata(&self.wallet_uuid, fingerprint_aliases, hws)
            .await
    }

    async fn send_wallet_invitation(&self, email: &str) -> Result<(), DaemonError> {
        let response = self
            .inner
            .request(
                Method::POST,
                &format!(
                    "{}/v1/wallets/{}/invitations",
                    self.inner.url, self.wallet_uuid
                ),
            )
            .await
            .json(&api::payload::CreateWalletInvitation { email })
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(DaemonError::Http(
                Some(response.status().into()),
                response.text().await?,
            ));
        }

        Ok(())
    }
}

fn history_tx_from_api(value: api::Transaction, network: Network) -> HistoryTransaction {
    let mut labels = HashMap::<String, Option<String>>::new();
    let mut coins = Vec::new();
    for input in &value.inputs {
        labels.insert(
            format!("{}:{}", input.txid, input.vout),
            input.label.clone(),
        );
        if input.kind == UTXOKind::Deposit || input.kind == UTXOKind::Change {
            if let Some(c) = &input.coin {
                coins.push(ListCoinsEntry {
                    address: c.address.clone(),
                    amount: c.amount,
                    derivation_index: c.derivation_index,
                    outpoint: c.outpoint,
                    block_height: c.block_height,
                    is_immature: c.is_immature,
                    is_change: c.is_change_address,
                    spend_info: c.spend_info.clone().map(|info| LCSpendInfo {
                        txid: info.txid,
                        height: info.height,
                    }),
                });
            }
        }
    }
    let mut changes_indexes = Vec::new();
    let txid = value.raw.txid().to_string();
    for (index, output) in value.outputs.iter().enumerate() {
        labels.insert(format!("{}:{}", txid, index), output.label.clone());
        if let Some(address) = &output.address {
            labels.insert(address.to_string(), output.address_label.clone());
        }
        if output.kind == UTXOKind::Deposit || output.kind == UTXOKind::Change {
            changes_indexes.push(index);
        }
    }
    labels.insert(txid, value.label);
    let mut tx = HistoryTransaction::new(
        value.raw,
        value.block_height,
        value.confirmed_at.map(|t| t as u32),
        coins,
        changes_indexes,
        network,
    );
    tx.load_labels(&labels);
    tx
}

fn spend_tx_from_api(
    value: api::Psbt,
    desc: &LianaDescriptor,
    secp: &secp256k1::Secp256k1<impl secp256k1::Verification>,
    network: Network,
) -> SpendTx {
    let mut labels = HashMap::<String, Option<String>>::new();
    let mut coins = Vec::new();
    for input in &value.inputs {
        labels.insert(
            format!("{}:{}", input.txid, input.vout),
            input.label.clone(),
        );
        if input.kind == UTXOKind::Deposit || input.kind == UTXOKind::Change {
            if let Some(c) = &input.coin {
                coins.push(ListCoinsEntry {
                    address: c.address.clone(),
                    amount: c.amount,
                    derivation_index: c.derivation_index,
                    outpoint: c.outpoint,
                    block_height: c.block_height,
                    is_immature: c.is_immature,
                    is_change: c.is_change_address,
                    spend_info: c.spend_info.clone().map(|info| LCSpendInfo {
                        txid: info.txid,
                        height: info.height,
                    }),
                });
            }
        }
    }
    let mut changes_indexes = Vec::new();
    let txid = value.raw.unsigned_tx.txid().to_string();
    for (index, output) in value.outputs.iter().enumerate() {
        labels.insert(format!("{}:{}", txid, index), output.label.clone());
        if let Some(address) = &output.address {
            labels.insert(address.to_string(), output.address_label.clone());
        }
        if output.kind == UTXOKind::Deposit || output.kind == UTXOKind::Change {
            changes_indexes.push(index);
        }
    }
    labels.insert(txid, value.label);
    let mut tx = SpendTx::new(
        Some(value.updated_at as u32),
        value.raw,
        coins,
        desc,
        secp,
        network,
    );
    tx.load_labels(&labels);
    tx
}

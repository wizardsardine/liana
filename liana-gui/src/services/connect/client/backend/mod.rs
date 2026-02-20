pub mod api;

use std::{
    collections::{HashMap, HashSet},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use async_trait::async_trait;
use chrono::Utc;
use liana::{
    descriptors::LianaDescriptor,
    miniscript::bitcoin::{
        address, bip32::ChildNumber, psbt::Psbt, Address, Network, OutPoint, Txid,
    },
};
use lianad::{
    bip329::Labels,
    commands::{CoinStatus, GetInfoDescriptors, LCSpendInfo, LabelItem, UpdateDerivIndexesResult},
    config::Config,
    payjoin::types::PayjoinStatus,
};
use reqwest::{Error, IntoUrl, Method, RequestBuilder};
use tokio::sync::RwLock;

use crate::{
    daemon::{model::*, Daemon, DaemonBackend, DaemonError},
    dir::LianaDirectory,
    hw::HardwareWalletConfig,
    services::connect::client::cache::ConnectCacheError,
};

use self::api::{UTXOKind, DEFAULT_OUTPOINTS_LIMIT};

use super::{
    auth::{self, AccessTokenResponse, AuthError},
    cache::update_connect_cache,
};

impl From<Error> for DaemonError {
    fn from(value: Error) -> Self {
        DaemonError::Http(None, serde_json::Value::String(value.to_string()))
    }
}

impl From<AuthError> for DaemonError {
    fn from(value: AuthError) -> Self {
        DaemonError::Http(value.http_status, serde_json::Value::String(value.error))
    }
}

fn request<U: IntoUrl>(
    http: &reqwest::Client,
    method: Method,
    url: U,
    access_token: &str,
    user_agent: &str,
) -> RequestBuilder {
    let req = http
        .request(method, url)
        .header("Authorization", format!("Bearer {}", access_token))
        .header("Content-Type", "application/json")
        .header("Liana-Version", format!("{}", crate::VERSION))
        .header("User-Agent", user_agent);
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
    unauthenticated: Arc<AtomicBool>,

    user_id: String,
}

impl BackendClient {
    pub async fn connect(
        auth_client: auth::AuthClient,
        url: String,
        credentials: auth::AccessTokenResponse,
        network: Network,
    ) -> Result<Self, DaemonError> {
        let http = reqwest::Client::new();
        let response = request(
            &http,
            Method::GET,
            format!("{}/v1/me", url),
            &credentials.access_token,
            auth_client.user_agent(),
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
            network,
            url,
            user_id,
            http,
            unauthenticated: Arc::new(AtomicBool::new(false)),
        })
    }

    pub fn auth_client(&self) -> &auth::AuthClient {
        &self.auth_client
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

    async fn request<F, D>(
        &self,
        method: Method,
        uri: &str,
        with_payload: F,
    ) -> Result<D, DaemonError>
    where
        F: FnOnce(RequestBuilder) -> RequestBuilder,
        D: serde::de::DeserializeOwned,
    {
        let access_token = &self.auth.read().await.access_token;
        let res = with_payload(request(
            &self.http,
            method,
            format!("{}{}", self.url, uri),
            access_token,
            self.auth_client.user_agent(),
        ))
        .send()
        .await?;

        let status = res.status();

        if status.is_success() {
            Ok(res.json().await?)
        } else {
            if status.as_u16() == 401 {
                self.unauthenticated.store(true, Ordering::Relaxed);
            }
            Err(DaemonError::Http(
                Some(status.as_u16()),
                res.json().await.unwrap_or_else(|_| {
                    serde_json::Value::String("Failed to read error response".to_string())
                }),
            ))
        }
    }

    async fn exec_request<F>(
        &self,
        method: Method,
        uri: &str,
        with_payload: F,
    ) -> Result<(), DaemonError>
    where
        F: FnOnce(RequestBuilder) -> RequestBuilder,
    {
        let access_token = &self.auth.read().await.access_token;
        let res = with_payload(request(
            &self.http,
            method,
            format!("{}{}", self.url, uri),
            access_token,
            self.auth_client.user_agent(),
        ))
        .send()
        .await?;

        let status = res.status();

        if status.is_success() {
            Ok(())
        } else {
            if status.as_u16() == 401 {
                self.unauthenticated.store(true, Ordering::Relaxed);
            }
            Err(DaemonError::Http(
                Some(status.as_u16()),
                res.json().await.unwrap_or_else(|_| {
                    serde_json::Value::String("Failed to read error response".to_string())
                }),
            ))
        }
    }

    pub async fn list_wallets(&self) -> Result<Vec<api::Wallet>, DaemonError> {
        let list_wallet: api::ListWallets = self.request(Method::GET, "/v1/wallets", |r| r).await?;

        Ok(list_wallet.wallets)
    }

    pub async fn create_wallet(
        &self,
        name: &str,
        descriptor: &LianaDescriptor,
        provider_keys: &Vec<api::payload::ProviderKey>,
    ) -> Result<api::Wallet, DaemonError> {
        self.request(Method::POST, "/v1/wallets", |r| {
            r.json(&api::payload::CreateWallet {
                name,
                descriptor,
                provider_keys,
            })
        })
        .await
    }

    pub async fn update_wallet_metadata(
        &self,
        wallet_uuid: &str,
        wallet_alias: Option<String>,
        fingerprint_aliases: &HashMap<Fingerprint, String>,
        hws: &[HardwareWalletConfig],
    ) -> Result<(), DaemonError> {
        let wallets = self.list_wallets().await?;
        let wallet = wallets
            .iter()
            .find(|w| w.id == wallet_uuid)
            .ok_or(DaemonError::Http(
                Some(404),
                serde_json::Value::String("No wallet exists for this uuid".to_string()),
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
                self.exec_request(
                    Method::PATCH,
                    &format!("/v1/wallets/{}", wallet_uuid),
                    |r| {
                        r.json(&api::payload::UpdateWallet {
                            alias: None,
                            ledger_hmac: Some(api::payload::UpdateLedgerHmac {
                                fingerprint: cfg.fingerprint.to_string(),
                                hmac: cfg.token.clone(),
                            }),
                            fingerprint_aliases: None,
                        })
                    },
                )
                .await?;
            }
        }

        let fingerprint_aliases: Option<Vec<api::payload::UpdateFingerprintAlias>> =
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
                Some(
                    fingerprint_aliases
                        .iter()
                        .map(|(fg, alias)| api::payload::UpdateFingerprintAlias {
                            fingerprint: fg.to_string(),
                            alias: alias.to_string(),
                        })
                        .collect(),
                )
            } else {
                None
            };

        if fingerprint_aliases.is_some() || wallet_alias.is_some() {
            self.exec_request(
                Method::PATCH,
                &format!("/v1/wallets/{}", wallet_uuid),
                |r| {
                    r.json(&api::payload::UpdateWallet {
                        alias: wallet_alias,
                        ledger_hmac: None,
                        fingerprint_aliases,
                    })
                },
            )
            .await?;
        }

        Ok(())
    }

    pub async fn get_wallet_invitation(
        &self,
        invitation_id: &str,
    ) -> Result<api::WalletInvitation, DaemonError> {
        self.request(
            Method::GET,
            &format!("/v1/invitations/{}", invitation_id),
            |r| r,
        )
        .await
    }

    pub async fn accept_wallet_invitation(&self, invitation_id: &str) -> Result<(), DaemonError> {
        self.exec_request(
            Method::POST,
            &format!("/v1/invitations/{}/accept", invitation_id),
            |r| r,
        )
        .await
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

    pub async fn get_wallet(&self) -> Result<api::Wallet, DaemonError> {
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
        self.inner
            .request(
                Method::GET,
                &format!("/v1/wallets/{}/psbts", self.wallet_uuid),
                |r| r.query(&query),
            )
            .await
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
        self.inner
            .request(
                Method::GET,
                &format!("/v1/wallets/{}/transactions", self.wallet_uuid),
                |r| r.query(&query),
            )
            .await
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
        self.inner
            .request(
                Method::GET,
                &format!("/v1/wallets/{}/transactions", self.wallet_uuid),
                |r| r.query(&query),
            )
            .await
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
        self.inner
            .request(
                Method::GET,
                &format!("/v1/wallets/{}/coins", self.wallet_uuid),
                |r| r.query(&query),
            )
            .await
    }

    pub async fn user_wallet_membership(&self) -> Result<api::UserRole, DaemonError> {
        let list: api::ListWalletMembers = self
            .inner
            .request(
                Method::GET,
                &format!("/v1/wallets/{}/members", self.wallet_uuid),
                |r| r,
            )
            .await?;
        list.members
            .into_iter()
            .find(|m| m.user_id == self.user_id())
            .map(|m| m.role)
            .ok_or(DaemonError::Unexpected("Membership not found".to_string()))
    }

    pub async fn auth(&self) -> AccessTokenResponse {
        self.inner.auth.read().await.clone()
    }

    pub async fn delete_wallet(&self) -> Result<(), DaemonError> {
        self.inner
            .exec_request(
                Method::DELETE,
                &format!("/v1/wallets/{}", self.wallet_uuid),
                |r| r,
            )
            .await
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
    async fn is_alive(
        &self,
        datadir: &LianaDirectory,
        network: Network,
    ) -> Result<(), DaemonError> {
        let auth = self.auth().await;
        if self.inner.unauthenticated.load(Ordering::Relaxed) {
            return Err(DaemonError::Http(
                Some(401),
                serde_json::Value::String("unauthenticated".to_string()),
            ));
        }
        if auth.expires_at < Utc::now().timestamp() + 60 {
            match self.inner.auth.try_write() {
                Err(_) => {
                    // something is using the lock, we will try next time.
                    return Ok(());
                }
                Ok(mut old) => {
                    let network_dir = datadir.network_directory(network);

                    let new = update_connect_cache(
                        &network_dir,
                        &old,
                        &self.inner.auth_client,
                        true, // refresh the token
                    )
                    .await
                    .map_err(|e| {
                        if let ConnectCacheError::Updating(AuthError { http_status, error }) = e {
                            DaemonError::Http(http_status, serde_json::Value::String(error))
                        } else {
                            DaemonError::Unexpected(format!(
                                "Cannot update Liana-connect cache: {}",
                                e
                            ))
                        }
                    })?;

                    *old = new;
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
            // We can ignore this field for remote backend as the wallet should remain synced.
            last_poll_timestamp: None,
            receive_index: wallet.deposit_derivation_index,
            change_index: wallet.change_derivation_index,
        })
    }

    async fn get_new_address(&self) -> Result<GetAddressResult, DaemonError> {
        let res: api::Address = self
            .inner
            .request(
                Method::POST,
                &format!("/v1/wallets/{}/addresses", self.wallet_uuid),
                |r| r,
            )
            .await?;

        Ok(GetAddressResult {
            address: res.address,
            derivation_index: res.derivation_index,
            bip21: None,
        })
    }

    async fn list_revealed_addresses(
        &self,
        is_change: bool,
        exclude_used: bool,
        limit: usize,
        start_index: Option<ChildNumber>,
    ) -> Result<ListRevealedAddressesResult, DaemonError> {
        let mut query = Vec::<(&str, String)>::new();
        query.push(("is_change_address", is_change.to_string()));
        query.push(("exclude_used", exclude_used.to_string()));
        query.push(("limit", limit.to_string()));
        if let Some(start) = start_index {
            query.push(("start_derivation_index", start.to_string()));
        }
        let res: api::ListRevealedAddresses = self
            .inner
            .request(
                Method::GET,
                &format!("/v1/wallets/{}/addresses", self.wallet_uuid),
                |r| r.query(&query),
            )
            .await?;

        Ok(ListRevealedAddressesResult {
            addresses: res
                .addresses
                .into_iter()
                .map(|addr| ListRevealedAddressesEntry {
                    index: addr.derivation_index,
                    address: addr.address,
                    label: addr.label,
                    used_count: addr.used_count,
                })
                .collect(),
            continue_from: res.continue_from,
        })
    }

    async fn receive_payjoin(&self) -> Result<GetAddressResult, DaemonError> {
        unimplemented!()
    }

    async fn get_payjoin_info(&self, _txid: &Txid) -> Result<PayjoinStatus, DaemonError> {
        unimplemented!()
    }

    async fn get_active_payjoin_sessions(&self) -> Result<Vec<u32>, DaemonError> {
        unimplemented!()
    }

    async fn update_deriv_indexes(
        &self,
        _receive: Option<u32>,
        _change: Option<u32>,
    ) -> Result<UpdateDerivIndexesResult, DaemonError> {
        Err(DaemonError::NotImplemented)
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
                    is_from_self: c.is_from_self,
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

        let res: Result<api::DraftPsbt, DaemonError> = self
            .inner
            .request(
                Method::POST,
                &format!("/v1/wallets/{}/psbts/generate", self.wallet_uuid),
                |r| {
                    r.json(&api::payload::GeneratePsbt {
                        save: false,
                        feerate: feerate_vb,
                        inputs: coins_outpoints,
                        recipients,
                    })
                },
            )
            .await;

        match res {
            Ok(draft) => Ok(CreateSpendResult::Success {
                psbt: draft.raw,
                warnings: draft.warnings,
            }),
            Err(DaemonError::Http(status, e)) => {
                let value: api::DraftPsbtErrors = serde_json::from_value(e).map_err(|e| {
                    DaemonError::Http(status, serde_json::Value::String(e.to_string()))
                })?;
                match value {
                    api::DraftPsbtErrors::InsufficientFunds(api::InsufficientFundsInfo {
                        missing,
                    }) => Ok(CreateSpendResult::InsufficientFunds { missing }),
                    api::DraftPsbtErrors::Error(api::DraftPsbtError { error }) => {
                        Err(DaemonError::Unexpected(error))
                    }
                }
            }
            Err(e) => Err(e),
        }
    }

    async fn rbf_psbt(
        &self,
        txid: &Txid,
        is_cancel: bool,
        feerate_vb: Option<u64>,
    ) -> Result<CreateSpendResult, DaemonError> {
        let res: Result<api::DraftPsbt, DaemonError> = self
            .inner
            .request(
                Method::POST,
                &format!("/v1/wallets/{}/psbts/rbf", self.wallet_uuid),
                |r| {
                    r.json(&api::payload::GenerateRbfPsbt {
                        txid: *txid,
                        is_cancel,
                        feerate: feerate_vb,
                        save: false,
                    })
                },
            )
            .await;

        match res {
            Ok(draft) => Ok(CreateSpendResult::Success {
                psbt: draft.raw,
                warnings: draft.warnings,
            }),
            Err(DaemonError::Http(status, e)) => {
                let value: api::DraftPsbtErrors = serde_json::from_value(e).map_err(|e| {
                    DaemonError::Http(status, serde_json::Value::String(e.to_string()))
                })?;
                match value {
                    api::DraftPsbtErrors::InsufficientFunds(api::InsufficientFundsInfo {
                        missing,
                    }) => Ok(CreateSpendResult::InsufficientFunds { missing }),
                    api::DraftPsbtErrors::Error(api::DraftPsbtError { error }) => {
                        Err(DaemonError::Unexpected(error))
                    }
                }
            }
            Err(e) => Err(e),
        }
    }

    async fn update_spend_tx(&self, psbt: &Psbt) -> Result<(), DaemonError> {
        self.inner
            .exec_request(
                Method::POST,
                &format!("/v1/wallets/{}/psbts", self.wallet_uuid),
                |r| {
                    r.json(&api::payload::ImportPsbt {
                        psbt: psbt.to_string(),
                    })
                },
            )
            .await
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
                serde_json::Value::String(format!("psbt not found with txid: {}", txid)),
            ))?;

        self.inner
            .exec_request(Method::DELETE, &format!("/v1/psbts/{}", psbt.uuid), |r| r)
            .await
    }

    async fn broadcast_spend_tx(&self, txid: &Txid) -> Result<(), DaemonError> {
        let psbt = self
            .list_psbts(&[*txid])
            .await?
            .psbts
            .into_iter()
            .find(|tx| tx.txid == *txid)
            .ok_or(DaemonError::NoAnswer)?;

        self.inner
            .exec_request(
                Method::POST,
                &format!("/v1/psbts/{}/broadcast", psbt.uuid),
                |r| r,
            )
            .await
    }

    async fn start_rescan(&self, _t: u32) -> Result<(), DaemonError> {
        Err(DaemonError::NoAnswer)
    }

    async fn create_recovery(
        &self,
        address: Address<address::NetworkUnchecked>,
        coins_outpoints: &[OutPoint],
        feerate_vb: u64,
        sequence: Option<u16>,
    ) -> Result<Psbt, DaemonError> {
        let timelock = sequence.ok_or(DaemonError::Unexpected("Missing sequence".to_string()))?;
        let res: api::DraftPsbt = self
            .inner
            .request(
                Method::POST,
                &format!("/v1/wallets/{}/psbts/recovery", self.wallet_uuid),
                |r| {
                    r.json(&api::payload::GenerateRecoveryPsbt {
                        save: false,
                        feerate: feerate_vb,
                        timelock,
                        address,
                        inputs: coins_outpoints,
                    })
                },
            )
            .await?;

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
            let wallet_labels: api::WalletLabels = self
                .inner
                .request(
                    Method::GET,
                    &format!("/v1/wallets/{}/labels", self.wallet_uuid),
                    |r| r.query(&[("items", chunk.join(","))]),
                )
                .await?;

            res.extend(wallet_labels.labels);
        }

        Ok(res)
    }

    async fn update_labels(
        &self,
        items: &HashMap<LabelItem, Option<String>>,
    ) -> Result<(), DaemonError> {
        self.inner
            .exec_request(
                Method::POST,
                &format!("/v1/wallets/{}/labels", self.wallet_uuid),
                |r| {
                    r.json(&api::payload::Labels {
                        labels: items
                            .iter()
                            .map(|(item, value)| api::payload::Label {
                                item: item.to_string(),
                                value: value.clone(),
                            })
                            .collect(),
                    })
                },
            )
            .await
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
        wallet_alias: Option<String>,
        fingerprint_aliases: &HashMap<Fingerprint, String>,
        hws: &[HardwareWalletConfig],
    ) -> Result<(), DaemonError> {
        self.inner
            .update_wallet_metadata(&self.wallet_uuid, wallet_alias, fingerprint_aliases, hws)
            .await
    }

    async fn send_wallet_invitation(&self, email: &str) -> Result<(), DaemonError> {
        self.inner
            .exec_request(
                Method::POST,
                &format!("/v1/wallets/{}/invitations", self.wallet_uuid),
                |r| r.json(&api::payload::CreateWalletInvitation { email }),
            )
            .await
    }

    async fn get_labels_bip329(&self, offset: u32, limit: u32) -> Result<Labels, DaemonError> {
        let res: api::Labels = self
            .inner
            .request(
                Method::GET,
                &format!(
                    "/v1/wallets/{}/labels/bip329?offset={}&limit={}",
                    self.wallet_uuid, offset, limit
                ),
                |r| r,
            )
            .await?;

        Ok(res.labels)
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
                    is_from_self: c.is_from_self,
                });
            }
        }
    }
    let mut changes_indexes = Vec::new();
    let txid = value.raw.compute_txid().to_string();
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
                    is_from_self: c.is_from_self,
                });
            }
        }
    }
    let mut changes_indexes = Vec::new();
    let txid = value.raw.unsigned_tx.compute_txid().to_string();
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
        None,
    );
    tx.load_labels(&labels);
    tx
}

///! Implementation of the Bitcoin interface using bitcoind.
///!
///! We use the RPC interface and a watchonly descriptor wallet.
use crate::config;

use std::{fs, io, time::Duration};

use jsonrpc::{
    arg,
    client::Client,
    simple_http::{self, SimpleHttpTransport},
};
use miniscript::{bitcoin, Descriptor, DescriptorPublicKey};

use serde_json::Value as Json;

// If bitcoind takes more than 3 minutes to answer one of our queries, fail.
const RPC_SOCKET_TIMEOUT: u64 = 180;

// Number of retries the client is allowed to do in case of timeout or i/o error
// while communicating with the bitcoin daemon.
// A retry happens every 1 second, this makes us give up after one minute.
const BITCOIND_RETRY_LIMIT: usize = 60;

// The minimum bitcoind version that can be used with revaultd.
const MIN_BITCOIND_VERSION: u64 = 239900;

/// An error in the bitcoind interface.
#[derive(Debug)]
pub enum BitcoindError {
    CookieFile(io::Error),
    /// Bitcoind server error.
    Server(jsonrpc::error::Error),
    /// They replied to a batch request omitting some responses.
    BatchMissingResponse,
    WalletCreation(String),
    DescriptorImport(String),
    WalletLoading(String),
    MissingOrTooManyWallet,
    InvalidVersion(u64),
    NetworkMismatch(String /*config*/, String /*bitcoind*/),
    MissingDescriptor,
}

impl BitcoindError {
    /// Is bitcoind just starting ?
    pub fn is_warming_up(&self) -> bool {
        match self {
            // https://github.com/bitcoin/bitcoin/blob/dca80ffb45fcc8e6eedb6dc481d500dedab4248b/src/rpc/protocol.h#L49
            BitcoindError::Server(jsonrpc::error::Error::Rpc(jsonrpc::error::RpcError {
                code,
                ..
            })) => *code == -28,
            _ => false,
        }
    }
}

impl std::fmt::Display for BitcoindError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            BitcoindError::CookieFile(e) => write!(f, "Reading bitcoind cookie file: {}", e),
            BitcoindError::Server(ref e) => write!(f, "Bitcoind RPC server error: {}", e),
            BitcoindError::BatchMissingResponse => write!(
                f,
                "Bitcoind server replied without enough responses to our batched request"
            ),
            BitcoindError::WalletCreation(s) => write!(f, "Error creating watchonly wallet: {}", s),
            BitcoindError::DescriptorImport(s) => write!(
                f,
                "Error importing descriptor. Response from bitcoind: '{}'",
                s
            ),
            BitcoindError::WalletLoading(s) => {
                write!(f, "Error when loading watchonly wallet: '{}'.", s)
            }
            BitcoindError::InvalidVersion(v) => {
                write!(
                    f,
                    "Invalid bitcoind version '{}', minimum supported is '{}'.",
                    v, MIN_BITCOIND_VERSION
                )
            }
            BitcoindError::NetworkMismatch(conf_net, bitcoind_net) => {
                write!(
                    f,
                    "Network mismatch. We are supposed to run on '{}' but bitcoind is on '{}'.",
                    conf_net, bitcoind_net
                )
            }
            BitcoindError::MissingOrTooManyWallet => {
                write!(
                    f,
                    "No, or too many, watchonly wallet(s) loaded on bitcoind."
                )
            }
            BitcoindError::MissingDescriptor => {
                write!(f, "The watchonly wallet loaded on bitcoind does not have the main descriptor imported.")
            }
        }
    }
}

impl std::error::Error for BitcoindError {}

impl From<jsonrpc::error::Error> for BitcoindError {
    fn from(e: jsonrpc::error::Error) -> Self {
        Self::Server(e)
    }
}

impl From<simple_http::Error> for BitcoindError {
    fn from(e: simple_http::Error) -> Self {
        jsonrpc::error::Error::Transport(Box::new(e)).into()
    }
}

pub struct BitcoinD {
    node_client: Client,
    watchonly_client: Client,
    watchonly_wallet_path: String,
    /// How many times we'll retry upon failure to send a request.
    retries: usize,
}

macro_rules! params {
    ($($param:expr),* $(,)?) => {
        [
            $(
                arg($param),
            )*
        ]
    };
}

impl BitcoinD {
    /// Create a new bitcoind interface. This tests the connection to bitcoind and disables retries
    /// on failure to send a request.
    pub fn new(
        config: &config::BitcoindConfig,
        watchonly_wallet_path: String,
    ) -> Result<BitcoinD, BitcoindError> {
        let cookie_string =
            fs::read_to_string(&config.cookie_path).map_err(BitcoindError::CookieFile)?;

        // Create a dummy client with a low timeout first to test the connection
        let dummy_node_client = Client::with_transport(
            SimpleHttpTransport::builder()
                .url(&config.addr.to_string())
                .map_err(BitcoindError::from)?
                .timeout(Duration::from_secs(3))
                .cookie_auth(cookie_string.clone())
                .build(),
        );
        let req = dummy_node_client.build_request("echo", &[]);
        dummy_node_client.send_request(req.clone())?;

        let node_client = Client::with_transport(
            SimpleHttpTransport::builder()
                .url(&config.addr.to_string())
                .map_err(BitcoindError::from)?
                .timeout(Duration::from_secs(RPC_SOCKET_TIMEOUT))
                .cookie_auth(cookie_string.clone())
                .build(),
        );

        // Create a dummy client with a low timeout first to test the connection
        let url = format!("http://{}/wallet/{}", config.addr, watchonly_wallet_path);
        let dummy_wo_client = Client::with_transport(
            SimpleHttpTransport::builder()
                .url(&url)
                .map_err(BitcoindError::from)?
                .timeout(Duration::from_secs(3))
                .cookie_auth(cookie_string.clone())
                .build(),
        );
        let req = dummy_wo_client.build_request("echo", &[]);
        dummy_wo_client.send_request(req.clone())?;

        let watchonly_url = format!("http://{}/wallet/{}", config.addr, watchonly_wallet_path);
        let watchonly_client = Client::with_transport(
            SimpleHttpTransport::builder()
                .url(&watchonly_url)
                .map_err(BitcoindError::from)?
                .timeout(Duration::from_secs(RPC_SOCKET_TIMEOUT))
                .cookie_auth(cookie_string.clone())
                .build(),
        );

        Ok(BitcoinD {
            node_client,
            watchonly_client,
            watchonly_wallet_path,
            retries: 0,
        })
    }

    /// Set how many times we'll retry a failed request. If passed None will set to default.
    pub fn with_retry_limit(mut self, retry_limit: Option<usize>) -> Self {
        self.retries = retry_limit.unwrap_or(BITCOIND_RETRY_LIMIT);
        self
    }

    /// Wrapper to retry a request sent to bitcoind upon IO failure
    /// according to the configured number of retries.
    fn retry<T, R: Fn() -> Result<T, BitcoindError>>(
        &self,
        request: R,
    ) -> Result<T, BitcoindError> {
        let mut error: Option<BitcoindError> = None;
        for i in 0..self.retries + 1 {
            match request() {
                Ok(res) => return Ok(res),
                Err(e) => {
                    if e.is_warming_up() {
                        error = Some(e)
                    } else if let BitcoindError::Server(jsonrpc::Error::Transport(ref err)) = e {
                        match err.downcast_ref::<simple_http::Error>() {
                            Some(simple_http::Error::Timeout)
                            | Some(simple_http::Error::SocketError(_))
                            | Some(simple_http::Error::HttpErrorCode(503)) => {
                                std::thread::sleep(Duration::from_secs(1));
                                log::debug!("Retrying RPC request to bitcoind: attempt #{}", i);
                                error = Some(e);
                            }
                            _ => return Err(e),
                        }
                    } else {
                        return Err(e);
                    }
                }
            }
        }

        Err(error.expect("Always set if we reach this point"))
    }

    fn make_request<'a, 'b>(
        &self,
        client: &Client,
        method: &'a str,
        params: &'b [Box<serde_json::value::RawValue>],
    ) -> Result<Json, BitcoindError> {
        self.retry(|| {
            let req = client.build_request(method, params);
            log::trace!("Sending to bitcoind: {:#?}", req);
            match client.send_request(req) {
                Ok(resp) => {
                    let res = resp.result().map_err(BitcoindError::Server)?;
                    log::trace!("Got from bitcoind: {:#?}", res);

                    return Ok(res);
                }
                Err(e) => Err(BitcoindError::Server(e)),
            }
        })
    }

    fn make_node_request(&self, method: &str, params: &[Box<serde_json::value::RawValue>]) -> Json {
        self.make_request(&self.node_client, method, params)
            .expect("We must not fail to make a request for more than a minute")
    }

    fn make_fallible_node_request(
        &self,
        method: &str,
        params: &[Box<serde_json::value::RawValue>],
    ) -> Result<Json, BitcoindError> {
        self.make_request(&self.node_client, method, params)
    }

    fn make_wallet_request(
        &self,
        method: &str,
        params: &[Box<serde_json::value::RawValue>],
    ) -> Json {
        self.make_request(&self.watchonly_client, method, params)
            .expect("We must not fail to make a request for more than a minute")
    }

    fn get_bitcoind_version(&self) -> u64 {
        self.make_node_request("getnetworkinfo", &[])
            .get("version")
            .map(Json::as_u64)
            .flatten()
            .expect("Missing or invalid 'version' in 'getnetworkinfo' result?")
    }

    fn get_network_bip70(&self) -> String {
        self.make_node_request("getblockchaininfo", &[])
            .get("chain")
            .map(Json::as_str)
            .flatten()
            .expect("Missing or invalid 'chain' in 'getblockchaininfo' result?")
            .to_string()
    }

    fn list_wallets(&self) -> Vec<String> {
        self.make_node_request("listwallets", &[])
            .as_array()
            .expect("API break, 'listwallets' didn't return an array.")
            .iter()
            .map(|json_str| {
                json_str
                    .as_str()
                    .expect("API break: 'listwallets' contains a non-string value")
                    .to_string()
            })
            .collect()
    }

    fn unload_wallet(&self, wallet_path: String) -> Option<String> {
        self.make_node_request("unloadwallet", &params!(Json::String(wallet_path),))
            .get("warning")
            .expect("No 'warning' in 'unloadwallet' response?")
            .as_str()
            .and_then(|w| {
                if w.is_empty() {
                    None
                } else {
                    Some(w.to_string())
                }
            })
    }

    fn create_wallet(&self, wallet_path: String) -> Option<String> {
        let res = self.make_node_request(
            "createwallet",
            &params!(
                Json::String(wallet_path),
                Json::Bool(true), // watchonly
                Json::Bool(true), // blank
            ),
        );

        if let Some(warning) = res.get("warning").map(Json::as_str).flatten() {
            if !warning.is_empty() {
                return Some(warning.to_string());
            }
        }
        if res.get("name").is_none() {
            return Some("Unknown error when create watchonly wallet".to_string());
        }

        None
    }

    // TODO: rescan feature will probably need another timestamp than 'now'
    fn import_descriptor(&self, descriptor: &Descriptor<DescriptorPublicKey>) -> Option<String> {
        let descriptors = vec![serde_json::json!({
            "desc": descriptor.to_string(),
            "timestamp": "now",
            "active": false,
        })];

        let res = self.make_wallet_request("importdescriptors", &params!(Json::Array(descriptors)));
        let all_succeeded = res
            .as_array()
            .map(|results| {
                results.iter().all(|res| {
                    res.get("success")
                        .map(Json::as_bool)
                        .flatten()
                        .unwrap_or(false)
                })
            })
            .unwrap_or(false);
        if all_succeeded {
            None
        } else {
            Some(res.to_string())
        }
    }

    fn list_descriptors(&self) -> Vec<String> {
        self.make_wallet_request("listdescriptors", &[])
            .get("descriptors")
            .and_then(Json::as_array)
            .expect("Missing or invalid 'descriptors' field in 'listdescriptors' response")
            .iter()
            .map(|elem| {
                elem.get("desc")
                    .and_then(Json::as_str)
                    .expect(
                        "Missing or invalid 'desc' field in 'listdescriptors' response's entries",
                    )
                    .to_string()
            })
            .collect::<Vec<String>>()
    }

    /// Create the watchonly wallet on bitcoind, and import it the main descriptor.
    pub fn create_watchonly_wallet(
        &self,
        main_descriptor: &Descriptor<DescriptorPublicKey>,
    ) -> Result<(), BitcoindError> {
        // Remove any leftover. This can happen if we delete the watchonly wallet but don't restart
        // bitcoind.
        while self.list_wallets().contains(&self.watchonly_wallet_path) {
            log::info!("Found a leftover watchonly wallet loaded on bitcoind. Removing it.");
            if let Some(e) = self.unload_wallet(self.watchonly_wallet_path.clone()) {
                log::error!(
                    "Unloading wallet '{}': '{}'",
                    &self.watchonly_wallet_path,
                    e
                );
            }
        }

        // Now create the wallet and import the main descriptor.
        if let Some(err) = self.create_wallet(self.watchonly_wallet_path.clone()) {
            return Err(BitcoindError::WalletCreation(err));
        }
        if let Some(err) = self.import_descriptor(main_descriptor) {
            return Err(BitcoindError::DescriptorImport(err));
        }

        Ok(())
    }

    pub fn maybe_load_watchonly_wallet(&self) -> Result<(), BitcoindError> {
        match self.make_fallible_node_request(
            "loadwallet",
            &params!(Json::String(self.watchonly_wallet_path.clone()),),
        ) {
            Err(e) => {
                if e.to_string().contains("is already loaded") {
                    Ok(())
                } else {
                    Err(e)
                }
            }
            Ok(res) => {
                if let Some(warning) = res.get("warning").map(Json::as_str).flatten() {
                    Err(BitcoindError::WalletLoading(warning.to_string()))
                } else if res.get("name").is_none() {
                    Err(BitcoindError::WalletLoading(res.to_string()))
                } else {
                    Ok(())
                }
            }
        }
    }

    /// Perform various sanity checks on the bitcoind instance.
    pub fn sanity_check(
        &self,
        main_descriptor: &Descriptor<DescriptorPublicKey>,
        config_network: bitcoin::Network,
    ) -> Result<(), BitcoindError> {
        // Check the minimum supported bitcoind version
        let version = self.get_bitcoind_version();
        if version < MIN_BITCOIND_VERSION {
            return Err(BitcoindError::InvalidVersion(version));
        }

        // Check bitcoind is running on the right network
        let bitcoind_net = self.get_network_bip70();
        let bip70_net = match config_network {
            bitcoin::Network::Bitcoin => "main",
            bitcoin::Network::Testnet => "test",
            bitcoin::Network::Regtest => "regtest",
            bitcoin::Network::Signet => "signet",
        };
        if bitcoind_net != bip70_net {
            return Err(BitcoindError::NetworkMismatch(
                bip70_net.to_string(),
                bitcoind_net,
            ));
        }

        // Check our watchonly wallet is loaded
        if self
            .list_wallets()
            .iter()
            .filter(|s| s == &&self.watchonly_wallet_path)
            .count()
            != 1
        {
            return Err(BitcoindError::MissingOrTooManyWallet);
        }

        // Check our main descriptor is imported in this wallet.
        if !self
            .list_descriptors()
            .contains(&main_descriptor.to_string())
        {
            return Err(BitcoindError::MissingDescriptor);
        }

        Ok(())
    }
}

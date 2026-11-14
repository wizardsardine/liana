use coincube_core::chain::ChainId;
use coincube_core::descriptors::CoincubeDescriptor;
use coincube_core::wallet_descriptor::{ImportedWalletDescriptor, WalletDescriptor};

use std::{fmt, net::SocketAddr, path::PathBuf, str::FromStr, time::Duration};

use crate::datadir::DataDirectory;
use miniscript::bitcoin::Network;

use serde::{de, Deserialize, Deserializer, Serialize, Serializer};

fn deserialize_fromstr<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: FromStr,
    <T as FromStr>::Err: std::fmt::Display,
{
    let string = String::deserialize(deserializer)?;
    T::from_str(&string)
        .map_err(|e| de::Error::custom(format!("Error parsing '{}': {}", string, e)))
}

pub fn serialize_to_string<T: std::fmt::Display, S: Serializer>(
    field: T,
    s: S,
) -> Result<S::Ok, S::Error> {
    s.serialize_str(&field.to_string())
}

#[derive(Deserialize)]
#[serde(untagged)]
enum StoredWalletDescriptor {
    /// Every pre-import daemon.toml stores one strict Vault descriptor string.
    Vault(String),
    Imported(ImportedWalletDescriptor),
}

fn deserialize_wallet_descriptor<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<WalletDescriptor, D::Error> {
    match StoredWalletDescriptor::deserialize(deserializer)? {
        StoredWalletDescriptor::Vault(value) => CoincubeDescriptor::from_str(&value)
            .map(WalletDescriptor::Vault)
            .map_err(de::Error::custom),
        StoredWalletDescriptor::Imported(value) => Ok(WalletDescriptor::Imported(value)),
    }
}

fn serialize_wallet_descriptor<S: Serializer>(
    descriptor: &WalletDescriptor,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    match descriptor {
        WalletDescriptor::Vault(value) => serializer.serialize_str(&value.to_string()),
        WalletDescriptor::Imported(value) => value.serialize(serializer),
    }
}

fn deserialize_duration<'de, D>(deserializer: D) -> Result<Duration, D::Error>
where
    D: Deserializer<'de>,
{
    let secs = u64::deserialize(deserializer)?;
    Ok(Duration::from_secs(secs))
}
pub fn serialize_duration<S: Serializer>(duration: &Duration, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_u64(duration.as_secs())
}

fn deserialize_rpc_auth<'de, D>(deserializer: D) -> Result<BitcoindRpcAuth, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    pub struct BitcoindRpcAuthHelper {
        cookie_path: Option<PathBuf>,
        auth: Option<String>,
    }
    let BitcoindRpcAuthHelper { cookie_path, auth } =
        BitcoindRpcAuthHelper::deserialize(deserializer)?;
    let rpc_auth = match (cookie_path, auth) {
        (Some(_), Some(_)) => {
            return Err(de::Error::custom(
                "must not set both `cookie_path` and `auth`",
            ));
        }
        (Some(path), None) => BitcoindRpcAuth::CookieFile(path),
        (None, Some(auth)) => auth
            .split_once(':')
            .ok_or(de::Error::custom("`auth` must be 'user:password'"))
            .map(|(user, pass)| BitcoindRpcAuth::UserPass(user.to_string(), pass.to_string()))?,
        (None, None) => {
            return Err(de::Error::custom("must set either `cookie_path` or `auth`"));
        }
    };
    Ok(rpc_auth)
}

fn serialize_userpass<S: Serializer>(
    user: &String,
    password: &String,
    s: S,
) -> Result<S::Ok, S::Error> {
    s.serialize_str(&format!("{}:{}", user, password))
}

fn default_loglevel() -> log::LevelFilter {
    log::LevelFilter::Info
}

/// Recommended `poll_interval_secs` for the Esplora backend. Picked
/// so the per-poll request volume fits comfortably inside public
/// Esplora providers' anonymous free tiers (mempool.space ≈ 60/min,
/// blockstream.info ≈ 700/hr). ~80-SPK wallets at this cadence land
/// at ~85 HTTP requests/hour against the active provider with the
/// smart-poll tip-guard active — well under any free-tier ceiling.
pub const ESPLORA_POLL_INTERVAL_SECS: u64 = 600;

/// Recommended `poll_interval_secs` for local-node backends
/// (`bitcoind`, `electrum`). These talk to a process you run on
/// localhost, so there's no per-request HTTP cost and no upstream
/// rate cap — a snappier cadence costs nothing and surfaces new
/// blocks/mempool activity to the UI within seconds.
pub const LOCAL_BACKEND_POLL_INTERVAL_SECS: u64 = 10;

fn default_poll_interval() -> Duration {
    // Used by serde when `poll_interval_secs` is missing from the
    // TOML. We can't tell which backend the config will end up
    // pointing at, so default to the Esplora-safe value
    // ([`ESPLORA_POLL_INTERVAL_SECS`]) — better to be a touch lazy
    // on bitcoind for one boot than to silently exceed an Esplora
    // free tier. The installer/migration in `coincube-gui` sets
    // a backend-appropriate value explicitly, so this serde
    // default only fires for hand-written or truncated configs.
    Duration::from_secs(ESPLORA_POLL_INTERVAL_SECS)
}

/// Bitcoin backend config.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub enum BitcoinBackend {
    /// Settings specific to bitcoind as the Bitcoin interface.
    #[serde(rename = "bitcoind_config")]
    Bitcoind(BitcoindConfig),
    /// Settings specific to Electrum as the Bitcoin interface.
    #[serde(rename = "electrum_config")]
    Electrum(ElectrumConfig),
    /// Settings specific to Esplora as the Bitcoin interface.
    #[serde(rename = "esplora_config")]
    Esplora(EsploraConfig),
}

/// RPC authentication options.
#[derive(Clone, PartialEq, Serialize)]
pub enum BitcoindRpcAuth {
    /// Path to bitcoind's cookie file.
    #[serde(rename = "cookie_path")]
    CookieFile(PathBuf),
    /// "USER:PASSWORD" for authentication.
    #[serde(rename = "auth", serialize_with = "serialize_userpass")]
    UserPass(String, String),
}

impl fmt::Debug for BitcoindRpcAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CookieFile(path) => path.fmt(f),
            Self::UserPass(_, _) => write!(f, "REDACTED RPC CREDENTIALS"),
        }
    }
}

/// Everything we need to know for talking to bitcoind serenely
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct BitcoindConfig {
    /// Authentication credentials for bitcoind's RPC server.
    #[serde(flatten, deserialize_with = "deserialize_rpc_auth")]
    pub rpc_auth: BitcoindRpcAuth,
    /// The IP:port bitcoind's RPC is listening on
    pub addr: SocketAddr,
}

/// Everything we need to know for talking to Electrum serenely.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct ElectrumConfig {
    /// The URL the Electrum's RPC is listening on.
    /// Include "ssl://" for SSL. otherwise TCP will be assumed.
    /// Can optionally prefix with "tcp://".
    pub addr: String,
    /// If validate_domain == false, domain of ssl certificate will not be validated
    /// (useful to allow usage of self signed certificates on local network)
    #[serde(default = "default_validate_domain")]
    pub validate_domain: bool,
}

fn default_validate_domain() -> bool {
    true
}

/// Everything we need to know for talking to an Esplora HTTP server.
///
/// Supports a primary endpoint plus up to two ordered fallbacks, tried in
/// declaration order on retryable failures. Each endpoint is independently
/// authenticated because typical pairings span auth domains — a public
/// Esplora (no token) as primary, a second public Esplora as
/// `fallback_addr`, and COINCUBE | Connect (JWT) as
/// `secondary_fallback_addr`.
///
/// The intent of the three-tier shape: primary + first fallback distribute
/// sync traffic across independent public providers (so a 429 on one doesn't
/// take the user to the metered backstop), and the secondary fallback
/// guarantees a route home for users whose IP has been rate-limited by both
/// public providers.
#[derive(Clone, PartialEq, Deserialize, Serialize)]
pub struct EsploraConfig {
    /// The HTTP(S) URL of the primary Esplora server.
    /// e.g. https://mempool.space/api
    pub addr: String,
    /// Optional JWT bearer token for the primary endpoint (e.g. COINCUBE | Connect).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub token: Option<String>,
    /// Optional first fallback Esplora endpoint, tried when the primary
    /// returns a retryable failure (transport error, 402, 429, 5xx). Typical
    /// deployment is a second public Esplora (`blockstream.info`) — using a
    /// different provider so a mempool 429 doesn't imply this one is also
    /// throttled.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub fallback_addr: Option<String>,
    /// Optional JWT bearer token for the first fallback endpoint.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub fallback_token: Option<String>,
    /// Optional second fallback Esplora endpoint, tried after the first
    /// fallback fails. Typical deployment is COINCUBE | Connect — the
    /// authenticated backstop, used only when both public providers have
    /// rejected the request.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub secondary_fallback_addr: Option<String>,
    /// Optional JWT bearer token for the second fallback endpoint.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub secondary_fallback_token: Option<String>,
}

impl EsploraConfig {
    /// Reject orphan tokens — a `fallback_token` without a
    /// matching `fallback_addr` (and likewise for the secondary
    /// pair) is almost certainly a typo or a partial hand-edit of
    /// the TOML; the daemon's `Client::new` would silently ignore
    /// the token because the whole provider is gated on the addr
    /// being present. We don't require the converse (addr without
    /// token is perfectly valid: anonymous public providers like
    /// mempool.space and blockstream.info are exactly that shape).
    fn check(&self) -> Result<(), ConfigError> {
        if self.fallback_token.is_some() && self.fallback_addr.is_none() {
            return Err(ConfigError::Unexpected(
                "esplora_config.fallback_token set without esplora_config.fallback_addr — \
                 the token would be silently ignored. Either add a fallback_addr or \
                 remove the orphan fallback_token."
                    .into(),
            ));
        }
        if self.secondary_fallback_token.is_some() && self.secondary_fallback_addr.is_none() {
            return Err(ConfigError::Unexpected(
                "esplora_config.secondary_fallback_token set without \
                 esplora_config.secondary_fallback_addr — the token would be silently \
                 ignored. Either add a secondary_fallback_addr or remove the orphan \
                 secondary_fallback_token."
                    .into(),
            ));
        }
        Ok(())
    }
}

impl std::fmt::Debug for EsploraConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EsploraConfig")
            .field("addr", &self.addr)
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .field("fallback_addr", &self.fallback_addr)
            .field(
                "fallback_token",
                &self.fallback_token.as_ref().map(|_| "<redacted>"),
            )
            .field("secondary_fallback_addr", &self.secondary_fallback_addr)
            .field(
                "secondary_fallback_token",
                &self.secondary_fallback_token.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

/// Settings for the Bitcoin interface.
///
/// On the wire this is `{ network, poll_interval_secs }`, exactly as it always was. The `network`
/// value is the chain *identity* (`ChainId`'s directory string): for the Bitcoin family that is
/// byte-for-byte what `bitcoin::Network` wrote — `"bitcoin"`, `"testnet"`, `"testnet4"`,
/// `"signet"`, `"regtest"` — and for Bitcoin Blake2b it is `"bitcoin-blake2b"` /
/// `"bitcoin-blake2b-testnet4"`, strings no previous build accepts. That is deliberate: an older
/// binary must *refuse* a fork configuration at parse time rather than run it as mainnet, which
/// is what an ignored optional key would have let it do (this struct never denied unknown fields).
///
/// The encoding [`Network`] (address bytes, HRP, descriptor and PSBT rules) is derived from the
/// identity and is never written to the file. It is still a plain field so that the many
/// encoding-only readers of `bitcoin_config.network` keep working unchanged; the pair must stay
/// consistent (see [`BitcoinConfig::check_chain_encoding`]).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(from = "BitcoinConfigWire", into = "BitcoinConfigWire")]
pub struct BitcoinConfig {
    /// The chain we are operating on: the identity that keys the data directory, the database
    /// and (later) the backend. Never derived from `network`.
    pub chain: ChainId,
    /// The encoding this chain uses, `chain.bitcoin_network()`. Not on the wire.
    pub network: Network,
    /// The poll interval for the Bitcoin interface
    pub poll_interval_secs: Duration,
}

impl BitcoinConfig {
    /// A configuration for `chain`, with the encoding derived from it.
    pub fn new(chain: ChainId, poll_interval_secs: Duration) -> Self {
        Self {
            chain,
            network: chain.bitcoin_network(),
            poll_interval_secs,
        }
    }

    /// The identity/encoding pair must agree. The pair is always consistent when it came through
    /// serde or [`BitcoinConfig::new`]; this exists for hand-built values (tests, the GUI's
    /// installer) so the daemon can refuse before touching anything on disk.
    pub fn check_chain_encoding(&self) -> Result<(), ConfigError> {
        let expected = self.chain.bitcoin_network();
        if self.network != expected {
            return Err(ConfigError::Unexpected(format!(
                "Chain '{}' encodes as network '{}' but the configuration carries network '{}'",
                self.chain, expected, self.network
            )));
        }
        Ok(())
    }
}

/// The on-disk shape of [`BitcoinConfig`]: the single `network` key carrying the chain identity,
/// plus the poll interval with its historical encoding and default.
#[derive(Deserialize, Serialize)]
struct BitcoinConfigWire {
    /// One of "bitcoin", "testnet", "testnet4", "signet", "regtest", "bitcoin-blake2b",
    /// "bitcoin-blake2b-testnet4". Anything else is a parse error, never a default.
    network: ChainId,
    #[serde(
        deserialize_with = "deserialize_duration",
        serialize_with = "serialize_duration",
        default = "default_poll_interval"
    )]
    poll_interval_secs: Duration,
}

impl From<BitcoinConfigWire> for BitcoinConfig {
    fn from(wire: BitcoinConfigWire) -> Self {
        BitcoinConfig::new(wire.network, wire.poll_interval_secs)
    }
}

impl From<BitcoinConfig> for BitcoinConfigWire {
    fn from(config: BitcoinConfig) -> Self {
        BitcoinConfigWire {
            network: config.chain,
            poll_interval_secs: config.poll_interval_secs,
        }
    }
}

/// Static informations we require to operate
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    #[cfg(feature = "native-recovery")]
    #[serde(skip)]
    native_recovery_pacing: Option<std::time::Duration>,
    /// legacy: An optional custom data directory
    data_dir: Option<PathBuf>,
    /// Current: A direct point to the current datadir
    data_directory: Option<PathBuf>,
    /// What messages to log
    #[serde(
        deserialize_with = "deserialize_fromstr",
        serialize_with = "serialize_to_string",
        default = "default_loglevel"
    )]
    pub log_level: log::LevelFilter,
    /// The descriptor to use for sending/receiving coins
    #[serde(
        deserialize_with = "deserialize_wallet_descriptor",
        serialize_with = "serialize_wallet_descriptor"
    )]
    pub main_descriptor: WalletDescriptor,
    /// Imported history lower bound; absent for strict Vaults. The database is
    /// authoritative after initialization and these values must match it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub import_birthday: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub import_initial_lookahead: Option<u32>,
    /// Settings for the Bitcoin interface
    pub bitcoin_config: BitcoinConfig,
    /// Settings specific to the Bitcoin backend.
    #[serde(flatten)]
    pub bitcoin_backend: Option<BitcoinBackend>,
    /// Optional Esplora (Connect) config used as a fallback when the active
    /// Bitcoind backend becomes unreachable.  Set at the moment the local node
    /// is promoted to primary; cleared if Connect is re-activated.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub fallback_esplora: Option<EsploraConfig>,
    /// Local Bitcoind config that will become the primary backend once IBD
    /// completes. Written when the background setup finishes (Connect is
    /// primary until then); cleared after the switch.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub pending_bitcoind: Option<BitcoindConfig>,
    /// Managed node selected during Connect-first Vault creation. The GUI
    /// provisions it after the Vault opens, then moves it to `pending_bitcoind`.
    /// Kept on disk so an interrupted download resumes on the next open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_managed_node_install: Option<String>,
    /// Whether the app should auto-switch to [`Self::pending_bitcoind`] as soon
    /// as it is synced.
    ///
    /// - `Some(true)` — the user adopted a node (installed alongside Connect, or
    ///   set one up in Settings); promote it once ready, even if it reused an
    ///   existing chainstate and was never observed in IBD.
    /// - `Some(false)` — a synced node is merely *parked* (the user switched to
    ///   Connect, or a Bitcoind failure fell back to Esplora); do NOT auto-revert.
    /// - `None` — key absent. Either a fresh config with no pending node, or a
    ///   *legacy* `daemon.toml` written before this flag existed, where any
    ///   `pending_bitcoind` was an adopted install-alongside node that the old
    ///   build promoted on IBD-completion with no flag. So an absent key is
    ///   treated as "promote" (see the auto-switch check), preserving that
    ///   behaviour across upgrades. Only the two explicit states are serialized,
    ///   keeping unaffected `daemon.toml` files clean.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_switch_to_pending: Option<bool>,
}

impl Config {
    /// Copy suitable for persistence: fork and imported-wallet Connect
    /// authentication are runtime-only. Legacy Vault persistence is unchanged.
    pub fn for_persistence(&self) -> Self {
        let mut config = self.clone();
        #[cfg(feature = "native-recovery")]
        {
            config.native_recovery_pacing = None;
        }
        if config.bitcoin_config.chain.is_blake2b()
            || config.main_descriptor.as_imported().is_some()
        {
            if let Some(backend) = config.fallback_esplora.as_mut() {
                backend.token = None;
                backend.fallback_token = None;
                backend.secondary_fallback_token = None;
            }
            if let Some(BitcoinBackend::Esplora(ref mut backend)) = config.bitcoin_backend {
                backend.token = None;
                backend.fallback_token = None;
                backend.secondary_fallback_token = None;
            }
        }
        config
    }

    pub fn new(
        bitcoin_config: BitcoinConfig,
        bitcoin_backend: Option<BitcoinBackend>,
        log_level: log::LevelFilter,
        main_descriptor: impl Into<WalletDescriptor>,
        data_directory: DataDirectory,
    ) -> Self {
        Self {
            #[cfg(feature = "native-recovery")]
            native_recovery_pacing: None,
            bitcoin_config,
            bitcoin_backend,
            log_level,
            main_descriptor: main_descriptor.into(),
            import_birthday: None,
            import_initial_lookahead: None,
            data_directory: Some(data_directory.path().to_path_buf()),
            data_dir: None,
            fallback_esplora: None,
            pending_bitcoind: None,
            pending_managed_node_install: None,
            auto_switch_to_pending: None,
        }
    }

    #[cfg(feature = "native-recovery")]
    pub fn set_native_recovery_pacing(
        &mut self,
        interval: std::time::Duration,
    ) -> Result<(), &'static str> {
        if self.bitcoin_config.chain != coincube_core::chain::ChainId::Testnet4
            || interval.is_zero()
            || interval > std::time::Duration::from_secs(2)
        {
            return Err("native_recovery_pacing_scope");
        }
        self.native_recovery_pacing = Some(interval);
        Ok(())
    }
    #[cfg(feature = "native-recovery")]
    pub(crate) fn native_recovery_pacing(&self) -> Option<std::time::Duration> {
        self.native_recovery_pacing
    }

    pub fn data_directory(&self) -> Option<DataDirectory> {
        if self.data_directory.is_some() {
            self.data_directory.clone().map(DataDirectory::new)
        } else if let Some(mut dir) = self.data_dir.clone() {
            dir.push(self.bitcoin_config.chain.dir_name());
            Some(DataDirectory::new(dir))
        } else {
            config_folder_path().map(|mut dir| {
                // Keyed on the identity, not the encoding: a Bitcoin Blake2b Cube must never
                // land in — or read from — `bitcoin/`. Byte-identical for the Bitcoin family.
                dir.push(self.bitcoin_config.chain.dir_name());
                DataDirectory::new(dir)
            })
        }
    }
}

#[derive(PartialEq, Eq, Debug)]
pub enum ConfigError {
    DatadirNotFound,
    FileNotFound,
    ReadingFile(String),
    UnexpectedDescriptor(Box<CoincubeDescriptor>),
    Unexpected(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match &self {
            Self::DatadirNotFound => write!(f, "Could not locate the configuration directory."),
            Self::FileNotFound => write!(f, "Could not locate the configuration file."),
            Self::ReadingFile(e) => write!(f, "Failed to read configuration file: {}", e),
            Self::UnexpectedDescriptor(desc) => write!(
                f,
                "Unexpected descriptor '{}'. We only support wsh() descriptors for now.",
                desc
            ),
            Self::Unexpected(e) => write!(f, "Configuration error: {}", e),
        }
    }
}

impl From<std::io::Error> for ConfigError {
    fn from(e: std::io::Error) -> Self {
        match e.kind() {
            std::io::ErrorKind::NotFound => Self::FileNotFound,
            _ => Self::ReadingFile(e.to_string()),
        }
    }
}

impl std::error::Error for ConfigError {}

/// Get the absolute path to the coincube configuration folder.
///
/// It's a "coincube/<network>/" directory in the XDG standard configuration directory for
/// all OSes but Linux-based ones, for which it's `~/.coincube/<network>/`.
/// There is only one config file at `coincube/config.toml`, which specifies the network.
/// Rationale: we want to have the database, RPC socket, etc.. in the same folder as the
/// configuration file but for Linux the XDG specifoes a data directory (`~/.local/share/`)
/// different from the configuration one (`~/.config/`).
pub fn config_folder_path() -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    let configs_dir = dirs::home_dir();

    #[cfg(not(target_os = "linux"))]
    let configs_dir = dirs::config_dir();

    if let Some(mut path) = configs_dir {
        #[cfg(target_os = "linux")]
        path.push(".coincube");

        #[cfg(not(target_os = "linux"))]
        path.push("Coincube");

        return Some(path);
    }

    None
}

fn config_file_path() -> Option<PathBuf> {
    config_folder_path().map(|mut path| {
        path.push("coincube.toml");
        path
    })
}

impl Config {
    /// Get our static configuration out of a mandatory configuration file.
    ///
    /// We require all settings to be set in the configuration file, and only in the configuration
    /// file. We don't allow to set them via the command line or environment variables to avoid a
    /// futile duplication.
    pub fn from_file(custom_path: Option<PathBuf>) -> Result<Config, ConfigError> {
        let config_file =
            custom_path.unwrap_or(config_file_path().ok_or(ConfigError::DatadirNotFound)?);

        let config = toml::from_slice::<Config>(&std::fs::read(config_file)?)
            .map_err(|e| ConfigError::ReadingFile(format!("Parsing configuration file: {}", e)))?;
        config.check()?;

        Ok(config)
    }

    /// Make sure the settings are sane.
    pub fn check(&self) -> Result<(), ConfigError> {
        // The identity and the encoding it implies must agree before either is trusted.
        self.bitcoin_config.check_chain_encoding()?;

        // Check the network of the xpubs in the descriptors
        let expected_network = match self.bitcoin_config.network {
            Network::Bitcoin => Network::Bitcoin,
            _ => Network::Testnet,
        };
        match &self.main_descriptor {
            WalletDescriptor::Vault(vault) => {
                if self.import_birthday.is_some() || self.import_initial_lookahead.is_some() {
                    return Err(ConfigError::Unexpected(
                        "Vault config cannot set imported scan options".into(),
                    ));
                }
                if !vault.all_xpubs_net_is(expected_network) {
                    return Err(ConfigError::Unexpected(format!(
                        "Our bitcoin network is {} but one xpub is not for network {}",
                        self.bitcoin_config.network, expected_network
                    )));
                }
            }
            WalletDescriptor::Imported(imported) => {
                if self.import_birthday.is_none()
                    || !self
                        .import_initial_lookahead
                        .is_some_and(|value| (1..=10_000).contains(&value))
                {
                    return Err(ConfigError::Unexpected(
                        "Imported wallet requires birthday and initial lookahead (1..10000)".into(),
                    ));
                }
                if imported.chain() != self.bitcoin_config.chain {
                    return Err(ConfigError::Unexpected(
                        "Imported wallet descriptor belongs to another chain".into(),
                    ));
                }
            }
        }

        // Validate Esplora endpoint pairs wherever they appear:
        // - The active backend when it's the Esplora variant.
        // - The legacy `fallback_esplora` slot used as a backup
        //   when a Bitcoind backend becomes unreachable.
        if let Some(BitcoinBackend::Esplora(e)) = &self.bitcoin_backend {
            e.check()?;
        }
        if let Some(e) = &self.fallback_esplora {
            e.check()?;
        }

        // TODO: check the semantics of the main descriptor

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    /// Helper: an `EsploraConfig` with only the mandatory `addr`
    /// set. Other fields default to `None`. Tests mutate the
    /// individual slots they care about.
    fn bare_esplora_config() -> EsploraConfig {
        EsploraConfig {
            addr: "https://mempool.space/api".into(),
            token: None,
            fallback_addr: None,
            fallback_token: None,
            secondary_fallback_addr: None,
            secondary_fallback_token: None,
        }
    }

    #[test]
    fn imported_connect_tokens_are_runtime_only_even_on_bitcoin_mainnet() {
        use coincube_core::miniscript::bitcoin::{
            bip32::{DerivationPath, Xpriv, Xpub},
            secp256k1::Secp256k1,
        };
        use std::str::FromStr;

        let secp = Secp256k1::new();
        let master = Xpriv::new_master(Network::Bitcoin, &[45; 32]).unwrap();
        let account = master
            .derive_priv(&secp, &DerivationPath::from_str("m/84h/0h/0h").unwrap())
            .unwrap();
        let key = format!(
            "[{}/84h/0h/0h]{}",
            master.fingerprint(&secp),
            Xpub::from_priv(&secp, &account)
        );
        let descriptor = ImportedWalletDescriptor::parse(
            &format!("wpkh({key}/0/*)"),
            Some(&format!("wpkh({key}/1/*)")),
            ChainId::Bitcoin,
        )
        .unwrap();
        let mut provider = bare_esplora_config();
        provider.token = Some("jwt-primary".into());
        provider.fallback_addr = Some("https://example.com/fallback".into());
        provider.fallback_token = Some("jwt-fallback".into());
        provider.secondary_fallback_addr = Some("https://example.com/secondary".into());
        provider.secondary_fallback_token = Some("jwt-secondary".into());
        let mut config = Config::new(
            BitcoinConfig::new(ChainId::Bitcoin, Duration::from_secs(30)),
            Some(BitcoinBackend::Esplora(provider.clone())),
            log::LevelFilter::Info,
            WalletDescriptor::Imported(descriptor),
            DataDirectory::new(std::env::temp_dir().join("imported-token-test")),
        );
        config.fallback_esplora = Some(provider);
        let serialized = toml::to_string(&config.for_persistence()).unwrap();
        assert!(!serialized.contains("jwt-"));
        assert!(serialized.contains("example.com/fallback"));
        assert!(serialized.contains("example.com/secondary"));
        // Authentication remains available to this process while the disk copy is public.
        assert!(toml::to_string(&config).unwrap().contains("jwt-primary"));
        // Pairing keeps the external wallet policy while rebinding its chain.
        let paired = config
            .main_descriptor
            .as_imported()
            .unwrap()
            .paired_blake2b()
            .unwrap();
        config.main_descriptor = WalletDescriptor::Imported(paired);
        config.bitcoin_config =
            BitcoinConfig::new(ChainId::BitcoinBlake2b, Duration::from_secs(30));
        config.import_birthday = Some(0);
        config.import_initial_lookahead = Some(20);
        config.check().unwrap();
        let restored: Config =
            toml::from_str(&toml::to_string(&config.for_persistence()).unwrap()).unwrap();
        assert_eq!(restored.main_descriptor, config.main_descriptor);
        config.bitcoin_config = BitcoinConfig::new(ChainId::Bitcoin, Duration::from_secs(30));
        assert!(config.check().is_err());
    }

    /// Regression: an orphan `fallback_token` (no matching
    /// `fallback_addr`) used to slide through the daemon's
    /// `Client::new` silently — the provider would be skipped and
    /// the token never used. Now rejected at config-load time.
    #[test]
    fn orphan_fallback_token_is_rejected() {
        let mut esp = bare_esplora_config();
        esp.fallback_token = Some("orphan-jwt".into());
        let err = esp.check().expect_err("orphan fallback_token must error");
        assert!(
            matches!(err, ConfigError::Unexpected(ref m) if m.contains("fallback_token")
                && m.contains("fallback_addr")),
            "error message must name both fields; got: {:?}",
            err,
        );
    }

    #[test]
    fn orphan_secondary_fallback_token_is_rejected() {
        let mut esp = bare_esplora_config();
        esp.secondary_fallback_token = Some("orphan-jwt".into());
        let err = esp
            .check()
            .expect_err("orphan secondary_fallback_token must error");
        assert!(
            matches!(err, ConfigError::Unexpected(ref m) if m.contains("secondary_fallback_token")
                && m.contains("secondary_fallback_addr")),
            "error message must name both fields; got: {:?}",
            err,
        );
    }

    /// Anonymous public providers (no auth) are the entire
    /// point of the fallback layer — addr with no token must
    /// stay valid. This is the converse case the bot's prompt
    /// almost asked us to reject; we deliberately don't.
    #[test]
    fn addr_without_token_is_valid() {
        let mut esp = bare_esplora_config();
        esp.fallback_addr = Some("https://blockstream.info/api".into());
        esp.secondary_fallback_addr =
            Some("https://api.coincube.io/api/v1/esplora/bitcoin/mainnet".into());
        esp.check()
            .expect("addr-only fallback shapes must validate");
    }

    /// A correctly-paired addr+token must validate, ensuring the
    /// rejection above is targeted at the orphan-token case
    /// rather than a blanket "token requires …" overreach.
    #[test]
    fn paired_addr_and_token_is_valid() {
        let mut esp = bare_esplora_config();
        esp.fallback_addr = Some("https://blockstream.info/api".into());
        esp.fallback_token = Some("legit-jwt".into());
        esp.secondary_fallback_addr =
            Some("https://api.coincube.io/api/v1/esplora/bitcoin/mainnet".into());
        esp.secondary_fallback_token = Some("legit-secondary-jwt".into());
        esp.check().expect("paired addr+token must validate");
    }

    // Test the format of the configuration file
    #[test]
    fn toml_config() {
        // A valid config
        let toml_str = r#"
            data_dir = "/home/wizardsardine/custom/folder/"
            log_level = "debug"
            main_descriptor = "wsh(andor(pk([aabbccdd]tpubDEN9WSToTyy9ZQfaYqSKfmVqmq1VVLNtYfj3Vkqh67et57eJ5sTKZQBkHqSwPUsoSskJeaYnPttHe2VrkCsKA27kUaN9SDc5zhqeLzKa1rr/<0;1>/*),older(10000),pk([aabbccdd]tpubD8LYfn6njiA2inCoxwM7EuN3cuLVcaHAwLYeups13dpevd3nHLRdK9NdQksWXrhLQVxcUZRpnp5CkJ1FhE61WRAsHxDNAkvGkoQkAeWDYjV/<0;1>/*)))#dw4ulnrs"

            [bitcoin_config]
            network = "bitcoin"
            poll_interval_secs = 18

            [bitcoind_config]
            cookie_path = "/home/user/.bitcoin/.cookie"
            addr = "127.0.0.1:8332"
            "#.trim_start().replace("            ", "");
        toml::from_str::<Config>(&toml_str).expect("Deserializing toml_str");

        // A valid, round-tripping, config
        {
            let toml_str = r#"
            data_dir = "/home/wizardsardine/custom/folder/"
            log_level = "TRACE"
            main_descriptor = "wsh(andor(pk([aabbccdd]tpubDEN9WSToTyy9ZQfaYqSKfmVqmq1VVLNtYfj3Vkqh67et57eJ5sTKZQBkHqSwPUsoSskJeaYnPttHe2VrkCsKA27kUaN9SDc5zhqeLzKa1rr/<0;1>/*),older(10000),pk([aabbccdd]tpubD8LYfn6njiA2inCoxwM7EuN3cuLVcaHAwLYeups13dpevd3nHLRdK9NdQksWXrhLQVxcUZRpnp5CkJ1FhE61WRAsHxDNAkvGkoQkAeWDYjV/<0;1>/*)))#dw4ulnrs"

            [bitcoin_config]
            network = "bitcoin"
            poll_interval_secs = 18

            [bitcoind_config]
            cookie_path = "/home/user/.bitcoin/.cookie"
            addr = "127.0.0.1:8332"
            "#.trim_start().replace("            ", "");
            let parsed = toml::from_str::<Config>(&toml_str).expect("Deserializing toml_str");
            let serialized = toml::to_string_pretty(&parsed).expect("Serializing to toml");
            assert_eq!(toml_str, serialized);
        }

        // A valid, round-tripping, config for a Taproot descriptor.
        {
            let toml_str = r#"
            data_dir = "/home/wizardsardine/custom/folder/"
            log_level = "TRACE"
            main_descriptor = "tr([abcdef01]xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/<0;1>/*,and_v(v:pk([abcdef01]xpub688Hn4wScQAAiYJLPg9yH27hUpfZAUnmJejRQBCiwfP5PEDzjWMNW1wChcninxr5gyavFqbbDjdV1aK5USJz8NDVjUy7FRQaaqqXHh5SbXe/<0;1>/*),older(52560)))#0mt7e93c"

            [bitcoin_config]
            network = "bitcoin"
            poll_interval_secs = 18

            [bitcoind_config]
            cookie_path = "/home/user/.bitcoin/.cookie"
            addr = "127.0.0.1:8332"
            "#.trim_start().replace("            ", "");
            let parsed = toml::from_str::<Config>(&toml_str).expect("Deserializing toml_str");
            let serialized = toml::to_string_pretty(&parsed).expect("Serializing to toml");
            assert_eq!(toml_str, serialized);
        }

        // A valid, round-tripping, config with `auth` instead of `cookie_path`
        {
            let toml_str = r#"
            data_dir = "/home/wizardsardine/custom/folder/"
            log_level = "TRACE"
            main_descriptor = "wsh(andor(pk([aabbccdd]tpubDEN9WSToTyy9ZQfaYqSKfmVqmq1VVLNtYfj3Vkqh67et57eJ5sTKZQBkHqSwPUsoSskJeaYnPttHe2VrkCsKA27kUaN9SDc5zhqeLzKa1rr/<0;1>/*),older(10000),pk([aabbccdd]tpubD8LYfn6njiA2inCoxwM7EuN3cuLVcaHAwLYeups13dpevd3nHLRdK9NdQksWXrhLQVxcUZRpnp5CkJ1FhE61WRAsHxDNAkvGkoQkAeWDYjV/<0;1>/*)))#dw4ulnrs"

            [bitcoin_config]
            network = "bitcoin"
            poll_interval_secs = 18

            [bitcoind_config]
            auth = "my_user:my_password"
            addr = "127.0.0.1:8332"
            "#.trim_start().replace("            ", "");
            let parsed = toml::from_str::<Config>(&toml_str).expect("Deserializing toml_str");
            let serialized = toml::to_string_pretty(&parsed).expect("Serializing to toml");
            assert_eq!(toml_str, serialized);
        }

        // Invalid desc checksum
        let toml_str = r#"
            log_level = "trace"
            data_dir = "/home/wizardsardine/custom/folder/"

            main_descriptor = "wsh(andor(pk([aabbccdd]tpubDEN9WSToTyy9ZQfaYqSKfmVqmq1VVLNtYfj3Vkqh67et57eJ5sTKZQBkHqSwPUsoSskJeaYnPttHe2VrkCsKA27kUaN9SDc5zhqeLzKa1rr/<0;1>/*),older(10000),pk([aabbccdd]tpubD8LYfn6njiA2inCoxwM7EuN3cuLVcaHAwLYeups13dpevd3nHLRdK9NdQksWXrhLQVxcUZRpnp5CkJ1FhE61WRAsHxDNAkvGkoQkAeWDYjV/<0;1>/*)))#y5wcna2e"

            [bitcoin_config]
            network = "bitcoin"
            poll_interval_secs = 18

            [bitcoind_config]
            cookie_path = "/home/user/.bitcoin/.cookie"
            addr = "127.0.0.1:8332"
        "#;
        let config_res: Result<Config, toml::de::Error> = toml::from_str(toml_str);
        config_res.expect_err("Deserializing an invalid toml_str");

        // Not enough parameters: missing the Bitcoin network
        let toml_str = r#"
            log_level = "trace"
            data_dir = "/home/wizardsardine/custom/folder/"

            # The main descriptor semantics aren't checked, yet.
            main_descriptor = ""

            [bitcoin_config]
            poll_interval_secs = 18

            [bitcoind_config]
            cookie_path = "/home/user/.bitcoin/.cookie"
            addr = "127.0.0.1:8332"
        "#;
        let config_res: Result<Config, toml::de::Error> = toml::from_str(toml_str);
        config_res.expect_err("Deserializing an invalid toml_str");
    }

    // A legacy `daemon.toml` (written before `auto_switch_to_pending` existed)
    // carries a `pending_bitcoind` but no flag. It must deserialize to `None`
    // (interpreted as "promote", preserving the old IBD-completion behaviour),
    // and the three states must round-trip without polluting clean configs.
    #[test]
    fn auto_switch_to_pending_legacy_and_states() {
        let legacy = r#"
            data_dir = "/home/user/folder/"
            log_level = "INFO"
            main_descriptor = "wsh(andor(pk([aabbccdd]tpubDEN9WSToTyy9ZQfaYqSKfmVqmq1VVLNtYfj3Vkqh67et57eJ5sTKZQBkHqSwPUsoSskJeaYnPttHe2VrkCsKA27kUaN9SDc5zhqeLzKa1rr/<0;1>/*),older(10000),pk([aabbccdd]tpubD8LYfn6njiA2inCoxwM7EuN3cuLVcaHAwLYeups13dpevd3nHLRdK9NdQksWXrhLQVxcUZRpnp5CkJ1FhE61WRAsHxDNAkvGkoQkAeWDYjV/<0;1>/*)))#dw4ulnrs"

            [bitcoin_config]
            network = "bitcoin"
            poll_interval_secs = 18

            [bitcoind_config]
            cookie_path = "/home/user/.bitcoin/.cookie"
            addr = "127.0.0.1:8332"

            [pending_bitcoind]
            cookie_path = "/home/user/.bitcoin/.cookie"
            addr = "127.0.0.1:8332"
            "#
        .trim_start()
        .replace("            ", "");
        let parsed = toml::from_str::<Config>(&legacy).expect("Deserializing legacy config");
        // Absent key => None => legacy adopted node => promoted by the GUI.
        assert_eq!(parsed.auto_switch_to_pending, None);
        // A None flag is never serialized, so a clean config stays clean.
        let serialized = toml::to_string_pretty(&parsed).expect("Serializing");
        assert!(!serialized.contains("auto_switch_to_pending"));
        assert!(!serialized.contains("pending_managed_node_install"));

        // The two explicit states are persisted and round-trip.
        let mut adopted = parsed.clone();
        adopted.auto_switch_to_pending = Some(true);
        adopted.pending_managed_node_install = Some("KnotsBlake2b".to_string());
        let s = toml::to_string_pretty(&adopted).expect("Serializing adopted");
        assert!(s.contains("auto_switch_to_pending = true"));
        assert!(s.contains("pending_managed_node_install = \"KnotsBlake2b\""));
        assert_eq!(
            toml::from_str::<Config>(&s)
                .expect("re-parse adopted")
                .auto_switch_to_pending,
            Some(true)
        );
        assert_eq!(
            toml::from_str::<Config>(&s)
                .expect("re-parse pending install")
                .pending_managed_node_install
                .as_deref(),
            Some("KnotsBlake2b")
        );

        let mut parked = parsed;
        parked.auto_switch_to_pending = Some(false);
        let s = toml::to_string_pretty(&parked).expect("Serializing parked");
        assert!(s.contains("auto_switch_to_pending = false"));
        assert_eq!(
            toml::from_str::<Config>(&s)
                .expect("re-parse parked")
                .auto_switch_to_pending,
            Some(false)
        );
    }

    // Test the format of the bitcoind_config section
    #[test]
    fn toml_bitcoind_config() {
        // A valid config with cookie_path
        let toml_str = r#"
            cookie_path = "/home/user/.bitcoin/.cookie"
            addr = "127.0.0.1:8332"
            "#
        .trim_start()
        .replace("            ", "");
        toml::from_str::<BitcoindConfig>(&toml_str).expect("Deserializing toml_str");
        let parsed = toml::from_str::<BitcoindConfig>(&toml_str).expect("Deserializing toml_str");
        let serialized = toml::to_string_pretty(&parsed).expect("Serializing to toml");
        assert_eq!(toml_str, serialized);
        assert_eq!(
            parsed.rpc_auth,
            BitcoindRpcAuth::CookieFile(PathBuf::from("/home/user/.bitcoin/.cookie"))
        );

        // A valid config with auth
        let toml_str = r#"
            auth = "my_user:my_password"
            addr = "127.0.0.1:8332"
            "#
        .trim_start()
        .replace("            ", "");
        toml::from_str::<BitcoindConfig>(&toml_str).expect("Deserializing toml_str");
        let parsed = toml::from_str::<BitcoindConfig>(&toml_str).expect("Deserializing toml_str");
        let serialized = toml::to_string_pretty(&parsed).expect("Serializing to toml");
        assert_eq!(toml_str, serialized);
        assert_eq!(
            parsed.rpc_auth,
            BitcoindRpcAuth::UserPass("my_user".to_string(), "my_password".to_string())
        );

        // Must not set both cookie_file and auth
        let toml_str = r#"
            cookie_path = '/home/user/.bitcoin/.cookie'
            auth = 'my_user:my_password'
            addr = '127.0.0.1:8332'
            "#
        .trim_start()
        .replace("            ", "");
        let config_err = toml::from_str::<BitcoindConfig>(&toml_str)
            .expect_err("Deserializing an invalid toml_str");
        assert!(config_err
            .to_string()
            .contains("must not set both `cookie_path` and `auth`"));

        // Missing RPC credentials
        let toml_str = r#"
            addr = '127.0.0.1:8332'
            "#
        .trim_start()
        .replace("            ", "");
        let config_err = toml::from_str::<BitcoindConfig>(&toml_str)
            .expect_err("Deserializing an invalid toml_str");
        assert!(config_err
            .to_string()
            .contains("must set either `cookie_path` or `auth`"));

        // Missing colon in auth
        let toml_str = r#"
            auth = 'my_usermy_password'
            addr = '127.0.0.1:8332'
            "#
        .trim_start()
        .replace("            ", "");
        let config_err = toml::from_str::<BitcoindConfig>(&toml_str)
            .expect_err("Deserializing an invalid toml_str");
        assert!(config_err
            .to_string()
            .contains("`auth` must be 'user:password'"));
    }

    // Test the format of the `electrum_config` section
    #[test]
    fn toml_electrum_config() {
        // A valid config with `validate_domain`
        let toml_str = r#"
            addr = "ssl://electrum.blockstream.info:60002"
            validate_domain = false
            "#
        .trim_start()
        .replace("            ", "");
        toml::from_str::<ElectrumConfig>(&toml_str).expect("Deserializing toml_str");
        let parsed = toml::from_str::<ElectrumConfig>(&toml_str).expect("Deserializing toml_str");
        let serialized = toml::to_string_pretty(&parsed).expect("Serializing to toml");
        assert_eq!(toml_str, serialized);
        let expected = ElectrumConfig {
            addr: "ssl://electrum.blockstream.info:60002".into(),
            validate_domain: false,
        };
        assert_eq!(parsed, expected,);

        // A valid config w/o `validate_domain`
        let toml_str = r#"
            addr = "ssl://electrum.blockstream.info:60002"
            "#
        .trim_start()
        .replace("            ", "");
        let parsed = toml::from_str::<ElectrumConfig>(&toml_str).expect("Deserializing toml_str");
        let expected = ElectrumConfig {
            addr: "ssl://electrum.blockstream.info:60002".into(),
            // `validate_domain` must default to true
            validate_domain: true,
        };
        assert_eq!(parsed, expected,);
    }

    #[test]
    fn config_directory() {
        let filepath = config_file_path().expect("Getting config file path");

        #[cfg(target_os = "linux")]
        {
            assert!(filepath.as_path().starts_with("/home/"));
            assert!(filepath.as_path().ends_with(".coincube/coincube.toml"));
        }

        #[cfg(target_os = "macos")]
        assert!(filepath
            .as_path()
            .ends_with("Library/Application Support/Coincube/coincube.toml"));

        #[cfg(target_os = "windows")]
        assert!(filepath
            .as_path()
            .ends_with(r#"AppData\Roaming\Coincube\coincube.toml"#));
    }

    // ── Chain identity on the wire (BTCB2 plan, coincube-api#292) ──────────────

    /// A `bitcoin_config` table as an older build wrote it, for `network`.
    fn bitcoin_config_toml(network: &str) -> String {
        format!("network = \"{}\"\npoll_interval_secs = 18\n", network)
    }

    #[test]
    fn legacy_network_values_parse_and_round_trip_byte_for_byte() {
        // What every existing daemon.toml contains, and what it must keep containing: the
        // identity is the Bitcoin-family one, the encoding is the network itself, and writing
        // the value back produces the same bytes an older build wrote.
        for (raw, network, chain) in [
            ("bitcoin", Network::Bitcoin, ChainId::Bitcoin),
            ("testnet", Network::Testnet, ChainId::Testnet),
            ("testnet4", Network::Testnet4, ChainId::Testnet4),
            ("signet", Network::Signet, ChainId::Signet),
            ("regtest", Network::Regtest, ChainId::Regtest),
        ] {
            let toml_str = bitcoin_config_toml(raw);
            let parsed = toml::from_str::<BitcoinConfig>(&toml_str).expect(raw);
            assert_eq!(parsed.chain, chain, "{}", raw);
            assert_eq!(parsed.network, network, "{}", raw);
            assert_eq!(parsed.poll_interval_secs, Duration::from_secs(18));
            parsed.check_chain_encoding().expect(raw);
            assert_eq!(toml::to_string(&parsed).expect(raw), toml_str, "{}", raw);
            // The pair the wire produces is the one `new` produces.
            let built = BitcoinConfig::new(chain, Duration::from_secs(18));
            assert_eq!(built.network, parsed.network);
            assert_eq!(toml::to_string(&built).expect(raw), toml_str);
        }
    }

    #[test]
    fn fork_network_values_carry_identity_and_project_to_their_encoding() {
        for (raw, network, chain) in [
            ("bitcoin-blake2b", Network::Bitcoin, ChainId::BitcoinBlake2b),
            (
                "bitcoin-blake2b-testnet4",
                Network::Testnet4,
                ChainId::BitcoinBlake2bTestnet4,
            ),
        ] {
            let toml_str = bitcoin_config_toml(raw);
            let parsed = toml::from_str::<BitcoinConfig>(&toml_str).expect(raw);
            assert_eq!(parsed.chain, chain, "{}", raw);
            assert_eq!(parsed.network, network, "{}", raw);
            parsed.check_chain_encoding().expect(raw);
            // Written back as the single key, with the fork string — never as the encoding.
            let serialized = toml::to_string(&parsed).expect(raw);
            assert_eq!(serialized, toml_str);
            assert!(!serialized.contains("chain"), "{}", serialized);
        }
    }

    #[test]
    fn fork_network_values_are_refused_by_the_legacy_parser() {
        // What an older binary does with a fork daemon.toml: its `network` field is a
        // `bitcoin::Network`, which does not know the fork strings, so the file fails to
        // parse — it is never run as mainnet.
        #[derive(Debug, Deserialize)]
        struct LegacyBitcoinConfig {
            #[allow(dead_code)]
            network: Network,
        }
        for raw in ["bitcoin-blake2b", "bitcoin-blake2b-testnet4"] {
            let err = toml::from_str::<LegacyBitcoinConfig>(&bitcoin_config_toml(raw))
                .expect_err("legacy parser accepted a fork string");
            assert!(err.to_string().contains("network"), "{}", err);
        }
        // …while the five legacy values still parse for it, of course.
        for raw in ["bitcoin", "testnet", "testnet4", "signet", "regtest"] {
            toml::from_str::<LegacyBitcoinConfig>(&bitcoin_config_toml(raw)).expect(raw);
        }
    }

    #[test]
    fn unknown_network_values_are_a_parse_error_never_a_default() {
        for raw in [
            "",
            "mainnet",
            "Bitcoin",
            "btcb2",
            "bitcoin_blake2b",
            "bitcoin-blake2b-signet",
        ] {
            let err = toml::from_str::<BitcoinConfig>(&bitcoin_config_toml(raw)).expect_err(raw);
            assert!(err.to_string().contains("network"), "{:?}: {}", raw, err);
        }
        // A missing key is missing, not defaulted either.
        assert!(toml::from_str::<BitcoinConfig>("poll_interval_secs = 18\n").is_err());
        // The poll interval keeps its historical default when absent.
        let parsed = toml::from_str::<BitcoinConfig>("network = \"bitcoin\"\n").unwrap();
        assert_eq!(
            parsed.poll_interval_secs,
            Duration::from_secs(ESPLORA_POLL_INTERVAL_SECS)
        );
    }

    #[test]
    fn an_inconsistent_hand_built_pair_is_refused_before_any_io() {
        let mut config = BitcoinConfig::new(ChainId::Bitcoin, Duration::from_secs(10));
        config.check_chain_encoding().unwrap();
        config.network = Network::Signet;
        match config.check_chain_encoding() {
            Err(ConfigError::Unexpected(msg)) => {
                assert!(msg.contains("bitcoin") && msg.contains("signet"), "{}", msg);
            }
            other => panic!("expected a configuration error, got {:?}", other),
        }
        // A fork identity with the wrong encoding is caught the same way.
        let mut fork = BitcoinConfig::new(ChainId::BitcoinBlake2bTestnet4, Duration::from_secs(10));
        assert_eq!(fork.network, Network::Testnet4);
        fork.network = Network::Bitcoin;
        assert!(fork.check_chain_encoding().is_err());
    }

    #[test]
    fn config_check_enforces_the_chain_encoding_pair() {
        let toml_str = r#"
            data_dir = "/home/wizardsardine/custom/folder/"
            log_level = "debug"
            main_descriptor = "wsh(andor(pk([aabbccdd]tpubDEN9WSToTyy9ZQfaYqSKfmVqmq1VVLNtYfj3Vkqh67et57eJ5sTKZQBkHqSwPUsoSskJeaYnPttHe2VrkCsKA27kUaN9SDc5zhqeLzKa1rr/<0;1>/*),older(10000),pk([aabbccdd]tpubD8LYfn6njiA2inCoxwM7EuN3cuLVcaHAwLYeups13dpevd3nHLRdK9NdQksWXrhLQVxcUZRpnp5CkJ1FhE61WRAsHxDNAkvGkoQkAeWDYjV/<0;1>/*)))#dw4ulnrs"

            [bitcoin_config]
            network = "signet"
            poll_interval_secs = 18

            [bitcoind_config]
            cookie_path = "/home/user/.bitcoin/.cookie"
            addr = "127.0.0.1:8332"
            "#.trim_start().replace("            ", "");
        let mut config = toml::from_str::<Config>(&toml_str).expect("Deserializing toml_str");
        config.check().expect("consistent config");
        config.bitcoin_config.network = Network::Bitcoin;
        assert!(matches!(config.check(), Err(ConfigError::Unexpected(_))));
    }

    #[test]
    fn data_directory_is_keyed_on_the_chain_identity() {
        let toml_str = |network: &str| {
            format!(
                r#"
            data_dir = "/home/wizardsardine/custom/folder/"
            log_level = "debug"
            main_descriptor = "wsh(andor(pk([aabbccdd]tpubDEN9WSToTyy9ZQfaYqSKfmVqmq1VVLNtYfj3Vkqh67et57eJ5sTKZQBkHqSwPUsoSskJeaYnPttHe2VrkCsKA27kUaN9SDc5zhqeLzKa1rr/<0;1>/*),older(10000),pk([aabbccdd]tpubD8LYfn6njiA2inCoxwM7EuN3cuLVcaHAwLYeups13dpevd3nHLRdK9NdQksWXrhLQVxcUZRpnp5CkJ1FhE61WRAsHxDNAkvGkoQkAeWDYjV/<0;1>/*)))#dw4ulnrs"

            [bitcoin_config]
            network = "{}"

            [bitcoind_config]
            cookie_path = "/home/user/.bitcoin/.cookie"
            addr = "127.0.0.1:8332"
            "#,
                network
            )
            .trim_start()
            .replace("            ", "")
        };
        // The Bitcoin family resolves to exactly the directories older builds used …
        for (raw, dir) in [
            ("bitcoin", "bitcoin"),
            ("testnet", "testnet"),
            ("testnet4", "testnet4"),
            ("signet", "signet"),
            ("regtest", "regtest"),
        ] {
            let config = toml::from_str::<Config>(&toml_str(raw)).expect(raw);
            let data_dir = config.data_directory().expect("legacy data_dir is set");
            assert_eq!(
                data_dir.path(),
                PathBuf::from("/home/wizardsardine/custom/folder/").join(dir),
                "{}",
                raw
            );
        }
        // … and a fork identity gets its own, never the encoding twin's.
        let config = toml::from_str::<Config>(&toml_str("bitcoin-blake2b")).unwrap();
        assert_eq!(
            config.data_directory().unwrap().path(),
            PathBuf::from("/home/wizardsardine/custom/folder/bitcoin-blake2b")
        );
        let config = toml::from_str::<Config>(&toml_str("bitcoin-blake2b-testnet4")).unwrap();
        assert_eq!(
            config.data_directory().unwrap().path(),
            PathBuf::from("/home/wizardsardine/custom/folder/bitcoin-blake2b-testnet4")
        );
    }
}

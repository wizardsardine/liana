use liana::descriptors::LianaDescriptor;

use std::{fmt, net::SocketAddr, path::PathBuf, str::FromStr, time::Duration};

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

fn default_poll_interval() -> Duration {
    Duration::from_secs(30)
}

/// Bitcoin backend config.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum BitcoinBackend {
    /// Settings specific to bitcoind as the Bitcoin interface.
    #[serde(rename = "bitcoind_config")]
    Bitcoind(BitcoindConfig),
    /// Settings specific to Electrum as the Bitcoin interface.
    #[serde(rename = "electrum_config")]
    Electrum(ElectrumConfig),
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
#[derive(Debug, Clone, Deserialize, Serialize)]
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

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BitcoinConfig {
    /// The network we are operating on, one of "bitcoin", "testnet", "regtest", "signet"
    pub network: Network,
    /// The poll interval for the Bitcoin interface
    #[serde(
        deserialize_with = "deserialize_duration",
        serialize_with = "serialize_duration",
        default = "default_poll_interval"
    )]
    pub poll_interval_secs: Duration,
}

/// Static informations we require to operate
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    /// An optional custom data directory
    pub data_dir: Option<PathBuf>,
    /// What messages to log
    #[serde(
        deserialize_with = "deserialize_fromstr",
        serialize_with = "serialize_to_string",
        default = "default_loglevel"
    )]
    pub log_level: log::LevelFilter,
    /// The descriptor to use for sending/receiving coins
    #[serde(
        deserialize_with = "deserialize_fromstr",
        serialize_with = "serialize_to_string"
    )]
    pub main_descriptor: LianaDescriptor,
    /// Settings for the Bitcoin interface
    pub bitcoin_config: BitcoinConfig,
    /// Settings specific to the Bitcoin backend.
    #[serde(flatten)]
    pub bitcoin_backend: Option<BitcoinBackend>,
}

impl Config {
    pub fn data_dir(&self) -> Option<PathBuf> {
        self.data_dir.clone().or_else(config_folder_path)
    }
}

#[derive(PartialEq, Eq, Debug)]
pub enum ConfigError {
    DatadirNotFound,
    FileNotFound,
    ReadingFile(String),
    UnexpectedDescriptor(Box<LianaDescriptor>),
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

/// Get the absolute path to the liana configuration folder.
///
/// It's a "liana/<network>/" directory in the XDG standard configuration directory for
/// all OSes but Linux-based ones, for which it's `~/.liana/<network>/`.
/// There is only one config file at `liana/config.toml`, which specifies the network.
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
        path.push(".liana");

        #[cfg(not(target_os = "linux"))]
        path.push("Liana");

        return Some(path);
    }

    None
}

fn config_file_path() -> Option<PathBuf> {
    config_folder_path().map(|mut path| {
        path.push("liana.toml");
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
        // Check the network of the xpubs in the descriptors
        let expected_network = match self.bitcoin_config.network {
            Network::Bitcoin => Network::Bitcoin,
            _ => Network::Testnet,
        };
        if !self.main_descriptor.all_xpubs_net_is(expected_network) {
            return Err(ConfigError::Unexpected(format!(
                "Our bitcoin network is {} but one xpub is not for network {}",
                self.bitcoin_config.network, expected_network
            )));
        }

        // TODO: check the semantics of the main descriptor

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

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
            data_dir = '/home/wizardsardine/custom/folder/'
            log_level = 'TRACE'
            main_descriptor = 'wsh(andor(pk([aabbccdd]tpubDEN9WSToTyy9ZQfaYqSKfmVqmq1VVLNtYfj3Vkqh67et57eJ5sTKZQBkHqSwPUsoSskJeaYnPttHe2VrkCsKA27kUaN9SDc5zhqeLzKa1rr/<0;1>/*),older(10000),pk([aabbccdd]tpubD8LYfn6njiA2inCoxwM7EuN3cuLVcaHAwLYeups13dpevd3nHLRdK9NdQksWXrhLQVxcUZRpnp5CkJ1FhE61WRAsHxDNAkvGkoQkAeWDYjV/<0;1>/*)))#dw4ulnrs'

            [bitcoin_config]
            network = 'bitcoin'
            poll_interval_secs = 18

            [bitcoind_config]
            cookie_path = '/home/user/.bitcoin/.cookie'
            addr = '127.0.0.1:8332'
            "#.trim_start().replace("            ", "");
            let parsed = toml::from_str::<Config>(&toml_str).expect("Deserializing toml_str");
            let serialized = toml::to_string_pretty(&parsed).expect("Serializing to toml");
            assert_eq!(toml_str, serialized);
        }

        // A valid, round-tripping, config for a Taproot descriptor.
        {
            let toml_str = r#"
            data_dir = '/home/wizardsardine/custom/folder/'
            log_level = 'TRACE'
            main_descriptor = 'tr([abcdef01]xpub6Eze7yAT3Y1wGrnzedCNVYDXUqa9NmHVWck5emBaTbXtURbe1NWZbK9bsz1TiVE7Cz341PMTfYgFw1KdLWdzcM1UMFTcdQfCYhhXZ2HJvTW/<0;1>/*,and_v(v:pk([abcdef01]xpub688Hn4wScQAAiYJLPg9yH27hUpfZAUnmJejRQBCiwfP5PEDzjWMNW1wChcninxr5gyavFqbbDjdV1aK5USJz8NDVjUy7FRQaaqqXHh5SbXe/<0;1>/*),older(52560)))#0mt7e93c'

            [bitcoin_config]
            network = 'bitcoin'
            poll_interval_secs = 18

            [bitcoind_config]
            cookie_path = '/home/user/.bitcoin/.cookie'
            addr = '127.0.0.1:8332'
            "#.trim_start().replace("            ", "");
            let parsed = toml::from_str::<Config>(&toml_str).expect("Deserializing toml_str");
            let serialized = toml::to_string_pretty(&parsed).expect("Serializing to toml");
            assert_eq!(toml_str, serialized);
        }

        // A valid, round-tripping, config with `auth` instead of `cookie_path`
        {
            let toml_str = r#"
            data_dir = '/home/wizardsardine/custom/folder/'
            log_level = 'TRACE'
            main_descriptor = 'wsh(andor(pk([aabbccdd]tpubDEN9WSToTyy9ZQfaYqSKfmVqmq1VVLNtYfj3Vkqh67et57eJ5sTKZQBkHqSwPUsoSskJeaYnPttHe2VrkCsKA27kUaN9SDc5zhqeLzKa1rr/<0;1>/*),older(10000),pk([aabbccdd]tpubD8LYfn6njiA2inCoxwM7EuN3cuLVcaHAwLYeups13dpevd3nHLRdK9NdQksWXrhLQVxcUZRpnp5CkJ1FhE61WRAsHxDNAkvGkoQkAeWDYjV/<0;1>/*)))#dw4ulnrs'

            [bitcoin_config]
            network = 'bitcoin'
            poll_interval_secs = 18

            [bitcoind_config]
            auth = 'my_user:my_password'
            addr = '127.0.0.1:8332'
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

    // Test the format of the bitcoind_config section
    #[test]
    fn toml_bitcoind_config() {
        // A valid config with cookie_path
        let toml_str = r#"
            cookie_path = '/home/user/.bitcoin/.cookie'
            addr = '127.0.0.1:8332'
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
            auth = 'my_user:my_password'
            addr = '127.0.0.1:8332'
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
            addr = 'ssl://electrum.blockstream.info:60002'
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
            addr = 'ssl://electrum.blockstream.info:60002'
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
            assert!(filepath.as_path().ends_with(".liana/liana.toml"));
        }

        #[cfg(target_os = "macos")]
        assert!(filepath
            .as_path()
            .ends_with("Library/Application Support/Liana/liana.toml"));

        #[cfg(target_os = "windows")]
        assert!(filepath
            .as_path()
            .ends_with(r#"AppData\Roaming\Liana\liana.toml"#));
    }
}

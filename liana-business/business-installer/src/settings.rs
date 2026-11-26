use liana_gui::{
    airgap::device::{save_device, AirgappedDevice},
    daemon::DaemonError,
    dir::NetworkDirectory,
    services::connect::client::{
        auth::AuthClient,
        backend::BackendClient,
        cache::{update_connect_cache, Account, ConnectCacheError},
        get_service_config, BackendType,
    },
};
use miniscript::bitcoin::Network;
use serde_json::{Map, Value};

const AIRGAPPED_SIGNERS: &str = "airgapped_signers";

#[derive(Debug, thiserror::Error)]
pub enum StoreDeviceError {
    #[error("Could not fetch the service configuration: {0}")]
    ServiceConfig(#[from] reqwest::Error),
    #[error("No saved credentials for {0}")]
    CredentialsMissing(String),
    #[error("Could not read the credentials: {0}")]
    Cache(ConnectCacheError),
    #[error("Remote backend error: {0}")]
    Backend(DaemonError),
    #[error("The stored settings are not a JSON object")]
    NotAnObject,
    #[error("The stored signing devices are unreadable: {0}")]
    Devices(#[from] serde_json::Error),
}

pub async fn store_airgapped_device(
    network: Network,
    network_dir: NetworkDirectory,
    email: String,
    device: AirgappedDevice,
) -> Result<(), StoreDeviceError> {
    let backend_type = BackendType::LianaBusiness(crate::VERSION);
    let config = get_service_config(network, backend_type).await?;
    let auth_client = AuthClient::new(
        config.auth_api_url,
        config.auth_api_public_key,
        email.clone(),
        backend_type.user_agent(),
    );
    let mut tokens = Account::from_cache_by_email(&network_dir, &email)
        .map_err(StoreDeviceError::Cache)?
        .ok_or_else(|| StoreDeviceError::CredentialsMissing(email.clone()))?
        .tokens;
    if tokens.is_expired() {
        tokens = update_connect_cache(&network_dir, &tokens, &auth_client, true, None)
            .await
            .map_err(StoreDeviceError::Cache)?;
    }
    let client = BackendClient::connect(auth_client, config.backend_api_url, tokens, network)
        .await
        .map_err(StoreDeviceError::Backend)?;

    let settings = client
        .fetch_settings()
        .await
        .map_err(StoreDeviceError::Backend)?;
    let settings = add_device(settings, device)?;
    client
        .store_settings(&settings)
        .await
        .map_err(StoreDeviceError::Backend)
}

fn add_device(settings: Option<Value>, device: AirgappedDevice) -> Result<Value, StoreDeviceError> {
    let mut settings = settings.unwrap_or_else(|| Value::Object(Map::new()));
    let fields = settings
        .as_object_mut()
        .ok_or(StoreDeviceError::NotAnObject)?;
    let mut devices: Vec<AirgappedDevice> = match fields.remove(AIRGAPPED_SIGNERS) {
        Some(devices) => serde_json::from_value(devices)?,
        None => Vec::new(),
    };
    save_device(&mut devices, device);
    fields.insert(AIRGAPPED_SIGNERS.to_owned(), serde_json::to_value(devices)?);
    Ok(settings)
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use liana_gui::airgap::device::{AirgappedDevice, Registration};
    use miniscript::bitcoin::bip32::Fingerprint;
    use serde_json::json;

    use crate::settings::{add_device, StoreDeviceError};

    fn device(vendor: &str, registration: Registration) -> AirgappedDevice {
        let mut device: AirgappedDevice = serde_json::from_value(json!({
            "fingerprint": "9f141cf0",
            "vendor": vendor,
            "model": "test signer",
            "version": { "major": 1, "minor": 2, "patch": 3, "flag": "stable" },
            "capabilities": 3,
        }))
        .unwrap();
        device.record(registration);
        device
    }

    #[test]
    fn a_device_goes_in_an_empty_blob() {
        let settings = add_device(None, device("Vendor", Registration::default())).unwrap();
        let devices: Vec<AirgappedDevice> =
            serde_json::from_value(settings["airgapped_signers"].clone()).unwrap();
        assert_eq!(devices.len(), 1);
        assert_eq!(
            devices[0].fingerprint,
            Fingerprint::from_str("9f141cf0").unwrap()
        );
    }

    #[test]
    fn the_rest_of_the_blob_and_a_known_device_are_kept() {
        let known = device(
            "Vendor",
            Registration {
                descriptor_checksum: Some("u768v50p".to_owned()),
                stored: Some(true),
                proof: None,
            },
        );
        let blob = json!({ "wallets": [{ "name": "kept" }], "airgapped_signers": [known] });
        let settings = add_device(Some(blob), device("Other", Registration::default())).unwrap();
        assert_eq!(settings["wallets"][0]["name"], "kept");
        let devices: Vec<AirgappedDevice> =
            serde_json::from_value(settings["airgapped_signers"].clone()).unwrap();
        assert_eq!(devices, vec![known]);
    }

    #[test]
    fn a_blob_that_is_not_an_object_is_refused() {
        assert!(matches!(
            add_device(Some(json!([])), device("Vendor", Registration::default())),
            Err(StoreDeviceError::NotAnObject)
        ));
    }
}

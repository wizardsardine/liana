use crate::ledger_lib::{
    bitcoin_app, list_installed_apps, query_via_websocket, DeviceInfo, BASE_SOCKET_URL,
};
use form_urlencoded::Serializer as UrlSerializer;
use ledger_transport_hidapi::hidapi::HidApi;
use ledger_transport_hidapi::TransportNativeHID;

pub fn device_info(ledger_api: &TransportNativeHID) -> Result<DeviceInfo, String> {
    DeviceInfo::new(ledger_api)
        .map_err(|e| format!("Error fetching device info: {}. Is the Ledger unlocked?", e))
}

pub fn ledger_api() -> Result<HidApi, String> {
    HidApi::new().map_err(|e| format!("Error initializing HDI api: {}.", e))
}

pub fn install_app(ledger_api: &TransportNativeHID, is_testnet: bool, force: bool) {
    // First of all make sure it's not already installed.
    println!("Querying installed apps. Please confirm on device.");
    let lowercase_app_name = if is_testnet {
        "bitcoin test"
    } else {
        "bitcoin"
    };
    let apps = match list_installed_apps(ledger_api) {
        Ok(a) => a,
        Err(_e) => {
            // TODO: send message
            return;
        }
    };
    if apps
        .iter()
        .any(|app| app.name.to_lowercase() == lowercase_app_name)
        && !force
    {
        // error!("Bitcoin app already installed. Use the update command to update it.");
        // TODO: send message
        return;
    }

    if let Ok(device_info) = device_info(ledger_api) {
        let bitcoin_app = match bitcoin_app(&device_info, is_testnet) {
            Ok(Some(a)) => a,
            Ok(None) => {
                // TODO: send message
                return;
                // error!("Could not get info about Bitcoin app.",)
            }
            Err(_e) => {
                // TODO: send message
                return;
                // error!("Error querying info about Bitcoin app: {}.", e)
            }
        };

        // Now install the app by connecting through their websocket thing to their HSM. Make sure to
        // properly escape the parameters in the request's parameter.
        let install_ws_url = UrlSerializer::new(format!("{}/install?", BASE_SOCKET_URL))
            .append_pair("targetId", &device_info.target_id.to_string())
            .append_pair("perso", &bitcoin_app.perso)
            .append_pair("deleteKey", &bitcoin_app.delete_key)
            .append_pair("firmware", &bitcoin_app.firmware)
            .append_pair("firmwareKey", &bitcoin_app.firmware_key)
            .append_pair("hash", &bitcoin_app.hash)
            .finish();
        println!("Querying installed apps. Please confirm on device.");
        if let Err(_e) = query_via_websocket(ledger_api, &install_ws_url) {
            // TODO: send message
            return;
            //     error!(
            //     "Got an error when installing Bitcoin app from Ledger's remote HSM: {}.",
            //     e
            // );
        }
        println!("Successfully installed the app.");
    }
}


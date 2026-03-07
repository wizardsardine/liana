use std::time::Duration;

use miniscript::{
    bitcoin::{secp256k1, Psbt, ScriptBuf, TxOut},
    psbt::PsbtExt,
};

use payjoin::{bitcoin::Amount, IntoUrl, OhttpKeys};
use reqwest::{header::ACCEPT, Proxy};

pub(crate) const OHTTP_RELAY: &str = "https://pj.bobspacebkk.com";
pub(crate) const PAYJOIN_DIRECTORY: &str = "https://payjo.in";

pub(crate) fn http_agent() -> reqwest::blocking::Client {
    reqwest::blocking::Client::new()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchOhttpKeysError {
    Reqwest(String),
    InvalidOhttpKeys(String),
    InvalidUrl(String),
    UrlParseError,
    UnexpectedStatusCode(reqwest::StatusCode),
}

impl std::error::Error for FetchOhttpKeysError {}
impl std::fmt::Display for FetchOhttpKeysError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

pub(crate) fn fetch_ohttp_keys(
    ohttp_relay: impl IntoUrl,
    payjoin_directory: impl IntoUrl,
) -> Result<OhttpKeys, FetchOhttpKeysError> {
    let payjoin_directory_str = payjoin_directory.as_str().to_string();
    let payjoin_directory_url = payjoin_directory
        .into_url()
        .map_err(|_| FetchOhttpKeysError::InvalidUrl(payjoin_directory_str.clone()))?
        .join("/.well-known/ohttp-gateway")
        .map_err(|_| FetchOhttpKeysError::UrlParseError)?;

    let ohttp_relay_str = ohttp_relay.as_str().to_string();
    let proxy = Proxy::all(
        ohttp_relay
            .into_url()
            .map_err(|_| FetchOhttpKeysError::InvalidUrl(ohttp_relay_str.clone()))?
            .as_str(),
    )
    .map_err(|e| FetchOhttpKeysError::Reqwest(e.to_string()))?;
    let client = reqwest::blocking::Client::builder()
        .proxy(proxy)
        .build()
        .map_err(|e| FetchOhttpKeysError::Reqwest(e.to_string()))?;
    let res = client
        .get(payjoin_directory_url)
        .header(ACCEPT, "application/ohttp-keys")
        .send()
        .map_err(|e| FetchOhttpKeysError::Reqwest(e.to_string()))?;
    validate_ohttp_keys_response(res)
}

fn validate_ohttp_keys_response(
    res: reqwest::blocking::Response,
) -> Result<OhttpKeys, FetchOhttpKeysError> {
    if !res.status().is_success() {
        return Err(FetchOhttpKeysError::UnexpectedStatusCode(res.status()));
    }

    let body = res.bytes().unwrap().to_vec();
    match OhttpKeys::decode(&body) {
        Ok(ohttp_keys) => Ok(ohttp_keys),
        Err(err) => Err(FetchOhttpKeysError::InvalidOhttpKeys(err.to_string())),
    }
}

pub(crate) fn post_request(
    req: payjoin::Request,
) -> Result<reqwest::blocking::Response, reqwest::Error> {
    let http = http_agent();
    http.post(req.url)
        .header("Content-Type", req.content_type)
        .body(req.body)
        .timeout(Duration::from_secs(10))
        .send()
}

/// Optimistically attempt to create witness for all inputs.
/// This method will not fail even if some inputs are not finalized or include invalid partial signatures.
pub(crate) fn finalize_psbt(psbt: &mut Psbt, secp: &secp256k1::Secp256k1<secp256k1::VerifyOnly>) {
    let mut witness_utxo_to_clean = vec![];
    let mut inputs_to_finalize = vec![];
    for (index, input) in psbt.inputs.iter_mut().enumerate() {
        if input.witness_utxo.is_none() {
            // Sender's wallet cleans this up (from original PSBT) but we need it to finalize_inp_mut() below
            input.witness_utxo = Some(TxOut {
                value: Amount::ZERO,
                script_pubkey: ScriptBuf::default(),
            });

            input.final_script_sig = None;
            input.final_script_witness = None;

            witness_utxo_to_clean.push(index);
            continue;
        }
        if input.final_script_sig.is_some()
            || input.final_script_witness.is_some()
            || input.partial_sigs.is_empty()
        {
            input.final_script_sig = None;
            input.final_script_witness = None;
            continue;
        }
        inputs_to_finalize.push(index);
    }

    for index in &inputs_to_finalize {
        match psbt.finalize_inp_mut(secp, *index) {
            Ok(_) => log::info!("Finalizing input at: {}", index),
            Err(e) => log::warn!("Failed to finalize input at: {} | {}", index, e),
        }
    }

    for index in witness_utxo_to_clean {
        psbt.inputs[index].witness_utxo = None;
    }
}

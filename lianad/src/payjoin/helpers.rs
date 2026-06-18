use std::path::Path;
use std::time::Duration;

use miniscript::{
    bitcoin::{secp256k1, Psbt, ScriptBuf, TxOut},
    psbt::PsbtExt,
};

use payjoin::{bitcoin::Amount, IntoUrl, OhttpKeys};
use reqwest::{header::ACCEPT, Proxy};

/// Fisher-Yates shuffle backed by `getrandom`.
pub(crate) fn shuffled<T: Clone>(input: &[T]) -> Vec<T> {
    let mut out = input.to_vec();
    let n = out.len();
    if n < 2 {
        return out;
    }
    for i in (1..n).rev() {
        let mut buf = [0u8; 8];
        getrandom::fill(&mut buf).expect("getrandom failure");
        let r = (u64::from_le_bytes(buf) as usize) % (i + 1);
        out.swap(i, r);
    }
    out
}

/// Outcome of one relay attempt: `Ok` succeeds, `Retry` advances to the next relay,
/// `Fatal` short-circuits with the given error (no further relays tried).
pub(crate) enum RelayAttempt<T, E> {
    Ok(T),
    Retry(E),
    Fatal(E),
}

/// Run `f` against a shuffled copy of `relays`, returning the first `Ok`.
/// `Retry` advances to the next relay; `Fatal` aborts immediately.
/// The list is randomized per call so different sessions/ticks pick different orders.
pub(crate) fn with_relay_fallback<T, E, F>(
    relays: &[String],
    empty_err: E,
    mut f: F,
) -> Result<T, E>
where
    F: FnMut(&str) -> RelayAttempt<T, E>,
{
    if relays.is_empty() {
        return Err(empty_err);
    }
    let order = shuffled(relays);
    let mut last_err: Option<E> = None;
    for relay in &order {
        match f(relay) {
            RelayAttempt::Ok(v) => return Ok(v),
            RelayAttempt::Fatal(e) => return Err(e),
            RelayAttempt::Retry(e) => {
                log::warn!("[Payjoin] relay {} failed (retryable), trying next", relay);
                last_err = Some(e);
            }
        }
    }
    Err(last_err.expect("non-empty relays guarantees at least one attempt"))
}

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
    NoRelays,
}

impl std::error::Error for FetchOhttpKeysError {}
impl std::fmt::Display for FetchOhttpKeysError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

pub(crate) fn fetch_ohttp_keys(
    ohttp_relays: &[String],
    payjoin_directory: impl IntoUrl,
    root_certificate: Option<&Path>,
) -> Result<OhttpKeys, FetchOhttpKeysError> {
    let payjoin_directory_str = payjoin_directory.as_str().to_string();
    let payjoin_directory_url = payjoin_directory
        .into_url()
        .map_err(|_| FetchOhttpKeysError::InvalidUrl(payjoin_directory_str.clone()))?
        .join("/.well-known/ohttp-gateway")
        .map_err(|_| FetchOhttpKeysError::UrlParseError)?
        .to_string();

    with_relay_fallback(ohttp_relays, FetchOhttpKeysError::NoRelays, |relay| {
        match fetch_ohttp_keys_once(relay, &payjoin_directory_url, root_certificate) {
            Ok(keys) => RelayAttempt::Ok(keys),
            Err(e @ FetchOhttpKeysError::Reqwest(_))
            | Err(e @ FetchOhttpKeysError::UnexpectedStatusCode(_))
            | Err(e @ FetchOhttpKeysError::InvalidUrl(_)) => RelayAttempt::Retry(e),
            Err(e) => RelayAttempt::Fatal(e),
        }
    })
}

fn fetch_ohttp_keys_once(
    ohttp_relay: &str,
    payjoin_directory_url: &str,
    root_certificate: Option<&Path>,
) -> Result<OhttpKeys, FetchOhttpKeysError> {
    let proxy = Proxy::all(ohttp_relay).map_err(|e| FetchOhttpKeysError::Reqwest(e.to_string()))?;
    let mut builder = reqwest::blocking::Client::builder()
        .proxy(proxy)
        .timeout(Duration::from_secs(15));
    if let Some(cert_path) = root_certificate {
        let cert_der = std::fs::read(cert_path)
            .map_err(|e| FetchOhttpKeysError::Reqwest(format!("failed to read cert: {e}")))?;
        let cert = reqwest::Certificate::from_der(&cert_der)
            .map_err(|e| FetchOhttpKeysError::Reqwest(format!("invalid cert: {e}")))?;
        builder = builder.add_root_certificate(cert);
    }
    let client = builder
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
    http.post(req.url.to_string())
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

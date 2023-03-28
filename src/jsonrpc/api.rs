use crate::{
    jsonrpc::{Error, Params, Request, Response},
    DaemonControl,
};

use std::{collections::HashMap, convert::TryInto, str::FromStr};

use miniscript::bitcoin::{self, consensus, util::psbt::PartiallySignedTransaction as Psbt};

fn create_spend(control: &DaemonControl, params: Params) -> Result<serde_json::Value, Error> {
    let destinations = params
        .get(0, "destinations")
        .ok_or_else(|| Error::invalid_params("Missing 'destinations' parameter."))?
        .as_object()
        .and_then(|obj| {
            obj.into_iter()
                .map(|(k, v)| {
                    let addr = bitcoin::Address::from_str(k).ok()?;
                    let amount: u64 = v.as_i64()?.try_into().ok()?;
                    Some((addr, amount))
                })
                .collect::<Option<HashMap<bitcoin::Address, u64>>>()
        })
        .ok_or_else(|| Error::invalid_params("Invalid 'destinations' parameter."))?;
    let outpoints = params
        .get(1, "outpoints")
        .ok_or_else(|| Error::invalid_params("Missing 'outpoints' parameter."))?
        .as_array()
        .and_then(|arr| {
            arr.iter()
                .map(|entry| {
                    entry
                        .as_str()
                        .and_then(|e| bitcoin::OutPoint::from_str(e).ok())
                })
                .collect::<Option<Vec<bitcoin::OutPoint>>>()
        })
        .ok_or_else(|| Error::invalid_params("Invalid 'outpoints' parameter."))?;
    let feerate: u64 = params
        .get(2, "feerate")
        .ok_or_else(|| Error::invalid_params("Missing 'feerate' parameter."))?
        .as_u64()
        .ok_or_else(|| Error::invalid_params("Invalid 'feerate' parameter."))?;

    let res = control.create_spend(&destinations, &outpoints, feerate)?;
    Ok(serde_json::json!(&res))
}

fn update_spend(control: &DaemonControl, params: Params) -> Result<serde_json::Value, Error> {
    let psbt: Psbt = params
        .get(0, "psbt")
        .ok_or_else(|| Error::invalid_params("Missing 'psbt' parameter."))?
        .as_str()
        .and_then(|s| base64::decode(s).ok())
        .and_then(|bytes| consensus::deserialize(&bytes).ok())
        .ok_or_else(|| Error::invalid_params("Invalid 'psbt' parameter."))?;
    control.update_spend(psbt)?;

    Ok(serde_json::json!({}))
}

fn delete_spend(control: &DaemonControl, params: Params) -> Result<serde_json::Value, Error> {
    let txid = params
        .get(0, "txid")
        .ok_or_else(|| Error::invalid_params("Missing 'txid' parameter."))?
        .as_str()
        .and_then(|s| bitcoin::Txid::from_str(s).ok())
        .ok_or_else(|| Error::invalid_params("Invalid 'txid' parameter."))?;
    control.delete_spend(&txid);

    Ok(serde_json::json!({}))
}

fn broadcast_spend(control: &DaemonControl, params: Params) -> Result<serde_json::Value, Error> {
    let txid = params
        .get(0, "txid")
        .ok_or_else(|| Error::invalid_params("Missing 'txid' parameter."))?
        .as_str()
        .and_then(|s| bitcoin::Txid::from_str(s).ok())
        .ok_or_else(|| Error::invalid_params("Invalid 'txid' parameter."))?;
    control.broadcast_spend(&txid)?;

    Ok(serde_json::json!({}))
}

fn list_confirmed(control: &DaemonControl, params: Params) -> Result<serde_json::Value, Error> {
    let start: u32 = params
        .get(0, "start")
        .ok_or_else(|| Error::invalid_params("Missing 'start' parameter."))?
        .as_i64()
        .and_then(|i| i.try_into().ok())
        .ok_or_else(|| Error::invalid_params("Invalid 'start' parameter."))?;

    let end: u32 = params
        .get(1, "end")
        .ok_or_else(|| Error::invalid_params("Missing 'end' parameter."))?
        .as_i64()
        .and_then(|i| i.try_into().ok())
        .ok_or_else(|| Error::invalid_params("Invalid 'end' parameter."))?;

    let limit: u64 = params
        .get(2, "limit")
        .ok_or_else(|| Error::invalid_params("Missing 'limit' parameter."))?
        .as_i64()
        .and_then(|i| i.try_into().ok())
        .ok_or_else(|| Error::invalid_params("Invalid 'limit' parameter."))?;

    Ok(serde_json::json!(
        &control.list_confirmed_transactions(start, end, limit)
    ))
}

fn list_transactions(control: &DaemonControl, params: Params) -> Result<serde_json::Value, Error> {
    let txids: Vec<bitcoin::Txid> = params
        .get(0, "txids")
        .ok_or_else(|| Error::invalid_params("Missing 'txids' parameter."))?
        .as_array()
        .and_then(|arr| {
            arr.iter()
                .map(|entry| entry.as_str().and_then(|e| bitcoin::Txid::from_str(e).ok()))
                .collect()
        })
        .ok_or_else(|| Error::invalid_params("Invalid 'txids' parameter."))?;
    Ok(serde_json::json!(&control.list_transactions(&txids)))
}

fn start_rescan(control: &DaemonControl, params: Params) -> Result<serde_json::Value, Error> {
    let timestamp: u32 = params
        .get(0, "timestamp")
        .ok_or_else(|| Error::invalid_params("Missing 'timestamp' parameter."))?
        .as_u64()
        .and_then(|t| t.try_into().ok())
        .ok_or_else(|| Error::invalid_params("Invalid 'timestamp' parameter."))?;
    control.start_rescan(timestamp)?;

    Ok(serde_json::json!({}))
}

fn create_recovery(control: &DaemonControl, params: Params) -> Result<serde_json::Value, Error> {
    let address = params
        .get(0, "address")
        .ok_or_else(|| Error::invalid_params("Missing 'address' parameter."))?
        .as_str()
        .and_then(|s| bitcoin::Address::from_str(s).ok())
        .ok_or_else(|| Error::invalid_params("Invalid 'address' parameter."))?;
    let feerate: u64 = params
        .get(1, "feerate")
        .ok_or_else(|| Error::invalid_params("Missing 'feerate' parameter."))?
        .as_u64()
        .ok_or_else(|| Error::invalid_params("Invalid 'feerate' parameter."))?;
    let timelock: Option<u16> = params
        .get(2, "timelock")
        .map(|tl| {
            tl.as_u64()
                .and_then(|tl| tl.try_into().ok())
                .ok_or_else(|| Error::invalid_params("Invalid 'timelock' parameter."))
        })
        .transpose()?;

    let res = control.create_recovery(address, feerate, timelock)?;
    Ok(serde_json::json!(&res))
}

/// Handle an incoming JSONRPC2 request.
pub fn handle_request(control: &DaemonControl, req: Request) -> Result<Response, Error> {
    let result = match req.method.as_str() {
        "broadcastspend" => {
            let params = req
                .params
                .ok_or_else(|| Error::invalid_params("Missing 'txid' parameter."))?;
            broadcast_spend(control, params)?
        }
        "createrecovery" => {
            let params = req.params.ok_or_else(|| {
                Error::invalid_params("Missing 'address' and 'feerate' parameters.")
            })?;
            create_recovery(control, params)?
        }
        "createspend" => {
            let params = req.params.ok_or_else(|| {
                Error::invalid_params(
                    "Missing 'outpoints', 'destinations' and 'feerate' parameters.",
                )
            })?;
            create_spend(control, params)?
        }
        "delspendtx" => {
            let params = req
                .params
                .ok_or_else(|| Error::invalid_params("Missing 'txid' parameter."))?;
            delete_spend(control, params)?
        }
        "getinfo" => serde_json::json!(&control.get_info()),
        "getnewaddress" => serde_json::json!(&control.get_new_address()),
        "listcoins" => serde_json::json!(&control.list_coins()),
        "listconfirmed" => {
            let params = req.params.ok_or_else(|| {
                Error::invalid_params(
                    "The 'listconfirmed' command requires 3 parameters: 'start', 'end' and 'limit'",
                )
            })?;
            list_confirmed(control, params)?
        }
        "listspendtxs" => serde_json::json!(&control.list_spend()),
        "listtransactions" => {
            let params = req.params.ok_or_else(|| {
                Error::invalid_params(
                    "The 'listtransactions' command requires 1 parameter: 'txids'",
                )
            })?;
            list_transactions(control, params)?
        }
        "startrescan" => {
            let params = req
                .params
                .ok_or_else(|| Error::invalid_params("Missing 'timestamp' parameter."))?;
            start_rescan(control, params)?
        }
        "stop" => serde_json::json!({}),
        "updatespend" => {
            let params = req
                .params
                .ok_or_else(|| Error::invalid_params("Missing 'psbt' parameter."))?;
            update_spend(control, params)?
        }
        _ => {
            return Err(Error::method_not_found());
        }
    };

    Ok(Response::success(req.id, result))
}

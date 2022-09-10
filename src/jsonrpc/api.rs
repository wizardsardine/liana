use crate::{
    jsonrpc::{Error, Params, Request, Response},
    DaemonControl,
};

use std::{collections::HashMap, convert::TryInto, str::FromStr};

use miniscript::bitcoin;

fn create_spend(control: &DaemonControl, params: Params) -> Result<serde_json::Value, Error> {
    let outpoints = params
        .get(0, "outpoints")
        .ok_or(Error::invalid_params("Missing 'outpoints' parameter."))?
        .as_array()
        .and_then(|arr| {
            arr.into_iter()
                .map(|entry| {
                    entry
                        .as_str()
                        .and_then(|e| bitcoin::OutPoint::from_str(&e).ok())
                })
                .collect::<Option<Vec<bitcoin::OutPoint>>>()
        })
        .ok_or(Error::invalid_params("Invalid 'outpoints' parameter."))?;
    let destinations = params
        .get(1, "destinations")
        .ok_or(Error::invalid_params("Missing 'destinations' parameter."))?
        .as_object()
        .and_then(|obj| {
            obj.into_iter()
                .map(|(k, v)| {
                    let addr = bitcoin::Address::from_str(&k).ok()?;
                    let amount: u64 = v.as_i64()?.try_into().ok()?;
                    Some((addr, amount))
                })
                .collect::<Option<HashMap<bitcoin::Address, u64>>>()
        })
        .ok_or(Error::invalid_params("Invalid 'destinations' parameter."))?;
    let feerate: u64 = params
        .get(2, "feerate")
        .ok_or(Error::invalid_params("Missing 'feerate' parameter."))?
        .as_i64()
        .and_then(|i| i.try_into().ok())
        .ok_or(Error::invalid_params("Invalid 'feerate' parameter."))?;

    let res = control.create_spend(&outpoints, &destinations, feerate)?;
    Ok(serde_json::json!(&res))
}

/// Handle an incoming JSONRPC2 request.
pub fn handle_request(control: &DaemonControl, req: Request) -> Result<Response, Error> {
    let result = match req.method.as_str() {
        "createspend" => {
            let params = req.params.ok_or(Error::invalid_params(
                "Missing 'outpoints', 'destinations' and 'feerate' parameters.",
            ))?;
            create_spend(control, params)?
        }
        "getinfo" => serde_json::json!(&control.get_info()),
        "getnewaddress" => serde_json::json!(&control.get_new_address()),
        "listcoins" => serde_json::json!(&control.list_coins()),
        "stop" => serde_json::json!({}),
        _ => {
            return Err(Error::method_not_found());
        }
    };

    Ok(Response::success(req.id, result))
}

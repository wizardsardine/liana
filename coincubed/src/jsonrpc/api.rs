use crate::{
    commands::{CoinStatus, LabelItem},
    jsonrpc::rpc::{Error, Params, Request, Response},
    DaemonControl,
};

use std::{
    collections::{HashMap, HashSet},
    convert::TryInto,
    str::FromStr,
};

use miniscript::bitcoin::{self, psbt::Psbt, Txid};

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
                .collect::<Option<HashMap<bitcoin::Address<bitcoin::address::NetworkUnchecked>, u64>>>()
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
    let change_address: Option<bitcoin::Address<bitcoin::address::NetworkUnchecked>> = params
        .get(3, "change_address")
        .map(|addr| {
            let addr_str = addr.as_str().ok_or_else(|| {
                Error::invalid_params("Invalid 'change_address' parameter: must be a string.")
            })?;
            bitcoin::Address::from_str(addr_str).map_err(|e| {
                Error::invalid_params(format!("Invalid 'change_address' parameter: {}.", e))
            })
        })
        .transpose()?;

    let res = control.create_spend(&destinations, &outpoints, feerate, change_address)?;
    Ok(serde_json::json!(&res))
}

fn update_spend(control: &DaemonControl, params: Params) -> Result<serde_json::Value, Error> {
    let psbt: Psbt = params
        .get(0, "psbt")
        .ok_or_else(|| Error::invalid_params("Missing 'psbt' parameter."))?
        .as_str()
        .and_then(|s| Psbt::from_str(s).ok())
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

fn rbf_psbt(control: &DaemonControl, params: Params) -> Result<serde_json::Value, Error> {
    let txid = params
        .get(0, "txid")
        .ok_or_else(|| Error::invalid_params("Missing 'txid' parameter."))?
        .as_str()
        .and_then(|s| bitcoin::Txid::from_str(s).ok())
        .ok_or_else(|| Error::invalid_params("Invalid 'txid' parameter."))?;
    let is_cancel: bool = params
        .get(1, "is_cancel")
        .ok_or_else(|| Error::invalid_params("Missing 'is_cancel' parameter."))?
        .as_bool()
        .ok_or_else(|| Error::invalid_params("Invalid 'is_cancel' parameter."))?;
    let feerate_vb: Option<u64> = if let Some(feerate) = params.get(2, "feerate") {
        Some(
            feerate
                .as_u64()
                .ok_or_else(|| Error::invalid_params("Invalid 'feerate' parameter."))?,
        )
    } else {
        None
    };
    let res = control.rbf_psbt(&txid, is_cancel, feerate_vb)?;
    Ok(serde_json::json!(&res))
}

fn list_coins(control: &DaemonControl, params: Option<Params>) -> Result<serde_json::Value, Error> {
    let statuses_arg = params
        .as_ref()
        .and_then(|p| p.get(0, "statuses"))
        .and_then(|statuses| statuses.as_array());
    let statuses: Vec<CoinStatus> = if let Some(statuses_arg) = statuses_arg {
        statuses_arg
            .iter()
            .map(|status_arg| {
                status_arg
                    .as_str()
                    .and_then(CoinStatus::from_arg)
                    .ok_or_else(|| {
                        Error::invalid_params(format!(
                            "Invalid value {} in 'statuses' parameter.",
                            status_arg
                        ))
                    })
            })
            .collect::<Result<Vec<CoinStatus>, Error>>()?
    } else {
        Vec::new()
    };
    let outpoints_arg = params
        .as_ref()
        .and_then(|p| p.get(1, "outpoints"))
        .and_then(|op| op.as_array());
    let outpoints: Vec<bitcoin::OutPoint> = if let Some(outpoints_arg) = outpoints_arg {
        outpoints_arg
            .iter()
            .map(|op_arg| {
                op_arg
                    .as_str()
                    .and_then(|op| bitcoin::OutPoint::from_str(op).ok())
                    .ok_or_else(|| {
                        Error::invalid_params(format!(
                            "Invalid value {} in 'outpoints' parameter.",
                            op_arg
                        ))
                    })
            })
            .collect::<Result<Vec<bitcoin::OutPoint>, Error>>()?
    } else {
        Vec::new()
    };
    let res = control.list_coins(&statuses, &outpoints);
    Ok(serde_json::json!(&res))
}

fn get_opt_u32<Q>(params: &Option<Params>, index: usize, name: &Q) -> Result<Option<u32>, Error>
where
    String: std::borrow::Borrow<Q>,
    Q: ?Sized + Ord + Eq + std::hash::Hash + std::fmt::Display,
{
    Ok(
        if let Some(i) = params.as_ref().and_then(|p| p.get(index, name)) {
            Some(i.as_u64().and_then(|i| i.try_into().ok()).ok_or_else(|| {
                Error::invalid_params(format!("Invalid value for '{}': {}", name, i))
            })?)
        } else {
            None
        },
    )
}

fn list_addresses(
    control: &DaemonControl,
    params: Option<Params>,
) -> Result<serde_json::Value, Error> {
    let start_index = get_opt_u32(&params, 0, "start_index")?;
    let count = get_opt_u32(&params, 1, "count")?;

    let res = &control.list_addresses(start_index, count)?;
    Ok(serde_json::json!(&res))
}

fn list_revealed_addresses(
    control: &DaemonControl,
    params: Params,
) -> Result<serde_json::Value, Error> {
    let is_change = params
        .get(0, "is_change")
        .ok_or_else(|| Error::invalid_params("Missing 'is_change' parameter."))?
        .as_bool()
        .ok_or_else(|| Error::invalid_params("Invalid 'is_change' parameter."))?;
    let exclude_used = params
        .get(1, "exclude_used")
        .ok_or_else(|| Error::invalid_params("Missing 'exclude_used' parameter."))?
        .as_bool()
        .ok_or_else(|| Error::invalid_params("Invalid 'exclude_used' parameter."))?;
    let limit = params
        .get(2, "limit")
        .ok_or_else(|| Error::invalid_params("Missing 'limit' parameter."))?
        .as_u64()
        .and_then(|l| l.try_into().ok())
        .ok_or_else(|| Error::invalid_params("Invalid 'limit' parameter."))?;
    // A missing value and `null` are both mapped to `None`.
    let start_index = if let Some(ind) = params.get(3, "start_index") {
        if ind.as_null().is_some() {
            None
        } else {
            let ind_u32: u32 = ind
                .as_u64()
                .and_then(|ind_u64| ind_u64.try_into().ok())
                .ok_or_else(|| Error::invalid_params("Invalid 'start_index' parameter."))?;
            Some(ind_u32)
        }
    } else {
        None
    };

    let res = &control.list_revealed_addresses(
        is_change,
        exclude_used,
        limit,
        start_index.map(|ind| ind.into()),
    )?;
    Ok(serde_json::json!(&res))
}

fn update_deriv_indexes(
    control: &DaemonControl,
    params: Params,
) -> Result<serde_json::Value, Error> {
    let receive = params.get(0, "receive");
    let change = params.get(1, "change");

    if receive.is_none() && change.is_none() {
        return Err(Error::invalid_params(
            "Missing 'receive' or 'change' parameter",
        ));
    }

    let receive = match receive {
        Some(i) => {
            let res = i.as_i64().ok_or(Error::invalid_params(
                "Invalid value for 'receive' param".to_string(),
            ))?;
            let res = res
                .try_into()
                .map_err(|_| Error::invalid_params("Invalid value for 'receive' param"))?;
            Some(res)
        }
        None => None,
    };

    let change = match change {
        Some(i) => {
            let res = i.as_i64().ok_or(Error::invalid_params(
                "Invalid value for 'change' param".to_string(),
            ))?;
            let res = res
                .try_into()
                .map_err(|_| Error::invalid_params("Invalid value for 'change' param"))?;
            Some(res)
        }
        None => None,
    };

    Ok(serde_json::json!(
        control.update_deriv_indexes(receive, change)?
    ))
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

fn list_spendtxs(
    control: &DaemonControl,
    params: Option<Params>,
) -> Result<serde_json::Value, Error> {
    let txids: Option<Vec<bitcoin::Txid>> = if let Some(p) = params {
        let tx_ids = p.get(0, "txids");
        if let Some(ids) = tx_ids {
            let ids: Vec<Txid> = ids
                .as_array()
                .and_then(|arr| {
                    arr.iter()
                        .map(|entry| entry.as_str().and_then(|e| bitcoin::Txid::from_str(e).ok()))
                        .collect()
                })
                .ok_or_else(|| Error::invalid_params("Invalid 'txids' parameter."))?;
            Some(ids)
        } else {
            None
        }
    } else {
        None
    };

    Ok(serde_json::json!(&control.list_spend(txids)?))
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

fn start_rescan(control: &mut DaemonControl, params: Params) -> Result<serde_json::Value, Error> {
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
    let outpoints = params
        .get(3, "outpoints")
        .map(|param| {
            param
                .as_array()
                .and_then(|arr| {
                    arr.iter()
                        .map(|entry| {
                            entry
                                .as_str()
                                .and_then(|e| bitcoin::OutPoint::from_str(e).ok())
                        })
                        .collect::<Option<Vec<_>>>()
                })
                .ok_or_else(|| Error::invalid_params("Invalid 'outpoints' parameter."))
        })
        .transpose()?
        .unwrap_or_default(); // missing is same as empty array

    let res = control.create_recovery(address, &outpoints, feerate, timelock)?;
    Ok(serde_json::json!(&res))
}

fn update_labels(control: &DaemonControl, params: Params) -> Result<serde_json::Value, Error> {
    let mut items = HashMap::new();
    for (item, value) in params
        .get(0, "labels")
        .ok_or_else(|| Error::invalid_params("Missing 'labels' parameter."))?
        .as_object()
        .ok_or_else(|| Error::invalid_params("Invalid 'labels' parameter."))?
        .iter()
    {
        let value = value.as_str().map(|s| s.to_string());
        if let Some(value) = &value {
            if value.len() > 100 {
                return Err(Error::invalid_params(format!(
                    "Invalid 'labels.{}' value length: must be less or equal than 100 characters",
                    item
                )));
            }
        }
        let item =
            LabelItem::from_str(item, control.config.bitcoin_config.network).ok_or_else(|| {
                Error::invalid_params(format!(
                    "Invalid 'labels.{}' parameter: must be an address, a txid or an outpoint",
                    item
                ))
            })?;
        items.insert(item, value);
    }

    control.update_labels(&items);
    Ok(serde_json::json!({}))
}

fn get_labels(control: &DaemonControl, params: Params) -> Result<serde_json::Value, Error> {
    let mut items = HashSet::new();
    for item in params
        .get(0, "items")
        .ok_or_else(|| Error::invalid_params("Missing 'items' parameter."))?
        .as_array()
        .ok_or_else(|| Error::invalid_params("Invalid 'items' parameter."))?
        .iter()
    {
        let item = item.as_str().ok_or_else(|| {
            Error::invalid_params(format!(
                "Invalid item {} format: must be an address, a txid or an outpoint",
                item
            ))
        })?;

        let item =
            LabelItem::from_str(item, control.config.bitcoin_config.network).ok_or_else(|| {
                Error::invalid_params(format!(
                    "Invalid item {} format: must be an address, a txid or an outpoint",
                    item
                ))
            })?;
        items.insert(item);
    }

    Ok(serde_json::json!(control.get_labels(&items)))
}

fn get_labels_bip329(control: &DaemonControl, params: Params) -> Result<serde_json::Value, Error> {
    let offset: u32 = params
        .get(0, "offset")
        .ok_or_else(|| Error::invalid_params("Missing 'offset' parameter."))?
        .as_u64()
        .and_then(|t| t.try_into().ok())
        .ok_or_else(|| Error::invalid_params("Invalid 'offset' parameter."))?;
    let limit: u32 = params
        .get(1, "limit")
        .ok_or_else(|| Error::invalid_params("Missing 'limit' parameter."))?
        .as_u64()
        .and_then(|t| t.try_into().ok())
        .ok_or_else(|| Error::invalid_params("Invalid 'limit' parameter."))?;
    Ok(serde_json::json!(control.get_labels_bip329(offset, limit)))
}

/// Handle an incoming JSONRPC2 request.
pub fn handle_request(control: &mut DaemonControl, req: Request) -> Result<Response, Error> {
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
        "rbfpsbt" => {
            let params = req.params.ok_or_else(|| {
                Error::invalid_params("Missing 'txid', 'feerate' and 'is_cancel' parameters.")
            })?;
            rbf_psbt(control, params)?
        }
        "getinfo" => serde_json::json!(&control.get_info()),
        "getnewaddress" => serde_json::json!(&control.get_new_address()),
        "updatederivationindexes" => {
            let params = req.params.ok_or_else(|| {
                Error::invalid_params("Missing 'receive' or 'change' parameters.")
            })?;
            update_deriv_indexes(control, params)?
        }
        "listcoins" => {
            let params = req.params;
            list_coins(control, params)?
        }
        "listaddresses" => {
            let params = req.params;
            list_addresses(control, params)?
        }
        "listrevealedaddresses" => {
            let params = req.params.ok_or_else(|| {
                Error::invalid_params(
                    "The 'listrevealedaddresses' command requires 3 parameters: 'is_change', 'exclude_used' and 'limit'",
                )
            })?;
            list_revealed_addresses(control, params)?
        }
        "listconfirmed" => {
            let params = req.params.ok_or_else(|| {
                Error::invalid_params(
                    "The 'listconfirmed' command requires 3 parameters: 'start', 'end' and 'limit'",
                )
            })?;
            list_confirmed(control, params)?
        }
        "listspendtxs" => list_spendtxs(control, req.params)?,
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
        "updatelabels" => {
            let params = req
                .params
                .ok_or_else(|| Error::invalid_params("Missing 'labels' parameter."))?;
            update_labels(control, params)?
        }
        "getlabels" => {
            let params = req
                .params
                .ok_or_else(|| Error::invalid_params("Missing 'items' parameter."))?;
            get_labels(control, params)?
        }
        "getlabelsbip329" => {
            let params = req
                .params
                .ok_or_else(|| Error::invalid_params("Missing 'offset' and 'limit' parameters."))?;
            get_labels_bip329(control, params)?
        }
        _ => {
            return Err(Error::method_not_found());
        }
    };

    Ok(Response::success(req.id, result))
}

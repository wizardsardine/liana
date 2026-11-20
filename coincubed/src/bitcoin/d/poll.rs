//! Fallible RPC reads used by polling. Transport failures are never absence.
use super::*;
mod parse;

impl BitcoinD {
    fn poll_request(
        &self,
        kind: ClientKind,
        method: &str,
        params: Option<&serde_json::value::RawValue>,
    ) -> Result<Json, BitcoindError> {
        use std::sync::atomic::Ordering;
        if self.poll_abort.load(Ordering::Relaxed) {
            return Err(BitcoindError::PollAborted);
        }
        let result = self.make_request_inner(kind, method, params, false);
        if self.poll_abort.load(Ordering::Relaxed) {
            return Err(BitcoindError::PollAborted);
        }
        result
    }

    pub(super) fn poll_node(
        &self,
        method: &str,
        params: Option<&serde_json::value::RawValue>,
    ) -> Result<Json, BitcoindError> {
        self.poll_request(ClientKind::PollNode, method, params)
    }

    fn poll_wallet(
        &self,
        method: &str,
        params: Option<&serde_json::value::RawValue>,
    ) -> Result<Json, BitcoindError> {
        self.poll_request(ClientKind::PollWallet, method, params)
    }

    /// The header time of the block `hash`, for the poller's published tip.
    /// Read by hash, so the time belongs to the tip the caller already holds,
    /// and through the poll client, which does not retry.
    pub fn try_block_time(&self, hash: &bitcoin::BlockHash) -> Result<u32, String> {
        let response = self
            .poll_node("getblockheader", params!(Json::String(hash.to_string()),))
            .map_err(|e| e.to_string())?;
        parse_block_stats(*hash, response)
            .map(|stats| stats.time)
            .map_err(|e| e.to_string())
    }

    pub fn try_get_block_hash(&self, height: i32) -> Result<Option<bitcoin::BlockHash>, String> {
        match self.poll_node("getblockhash", params!(Json::Number(height.into()))) {
            Ok(value) => value
                .as_str()
                .ok_or_else(|| "Invalid block hash response".to_string())?
                .parse()
                .map(Some)
                .map_err(|e: bitcoin::hashes::hex::HexToArrayError| e.to_string()),
            Err(BitcoindError::Server(jsonrpc::Error::Rpc(jsonrpc::error::RpcError {
                code: -8,
                ..
            }))) => Ok(None),
            Err(error) => Err(error.to_string()),
        }
    }

    pub fn try_list_since_block(&self, hash: &bitcoin::BlockHash) -> Result<LSBlockRes, String> {
        self.poll_wallet(
            "listsinceblock",
            params!(
                Json::String(hash.to_string()),
                Json::Number(1.into()),
                Json::Bool(true),
                Json::Bool(false),
                Json::Bool(true)
            ),
        )
        .map_err(|e| e.to_string())
        .and_then(parse::coins)
    }

    pub fn try_get_transaction(&self, txid: &bitcoin::Txid) -> Result<Option<GetTxRes>, String> {
        match self.poll_wallet("gettransaction", params!(Json::String(txid.to_string()))) {
            Ok(value) => parse::transaction(value, *txid).map(Some),
            Err(error) if error.is_unknown_to_wallet() => Ok(None),
            Err(error) => Err(error.to_string()),
        }
    }

    pub fn try_is_spent(&self, outpoint: &bitcoin::OutPoint) -> Result<bool, String> {
        let value = self
            .poll_node(
                "gettxout",
                params!(
                    Json::String(outpoint.txid.to_string()),
                    Json::Number(outpoint.vout.into())
                ),
            )
            .map_err(|e| e.to_string())?;
        if value.is_null() {
            return Ok(true);
        }
        if value.get("bestblock").and_then(Json::as_str).is_some() {
            return Ok(false);
        }
        Err("Invalid gettxout response".into())
    }

    pub fn try_is_in_mempool(&self, txid: &bitcoin::Txid) -> Result<bool, String> {
        match self.poll_node("getmempoolentry", params!(Json::String(txid.to_string()))) {
            Ok(value) if value.is_object() => Ok(true),
            Ok(_) => Err("Invalid getmempoolentry response".into()),
            Err(error) if error.is_unknown_to_wallet() => Ok(false),
            Err(error) => Err(error.to_string()),
        }
    }

    pub fn try_rescan_progress(&self) -> Result<Option<f64>, String> {
        let value = self
            .poll_wallet("getwalletinfo", None)
            .map_err(|e| e.to_string())?;
        match value.get("scanning") {
            Some(Json::Bool(false)) => Ok(None),
            Some(Json::Object(scan)) => scan
                .get("progress")
                .and_then(Json::as_f64)
                .filter(|p| p.is_finite() && (0.0..=1.0).contains(p))
                .map(Some)
                .ok_or_else(|| "Invalid rescan progress".into()),
            _ => Err("Missing or invalid scanning state".into()),
        }
    }

    pub fn try_get_spender_txid(
        &self,
        outpoint: &bitcoin::OutPoint,
    ) -> Result<Option<bitcoin::Txid>, String> {
        let Some(parent) = self.try_get_transaction(&outpoint.txid)? else {
            log::error!("{}", unknown_spent_coin_message(&outpoint.txid, outpoint));
            return Ok(None);
        };
        let height = match parent.block {
            Some(block) => block.height,
            None => self.try_chain_tip()?.height,
        };
        let Some(hash) = self.try_get_block_hash(height.saturating_sub(1))? else {
            return Ok(None);
        };
        let value = self
            .poll_wallet(
                "listsinceblock",
                params!(
                    Json::String(hash.to_string()),
                    Json::Number(1.into()),
                    Json::Bool(true),
                    Json::Bool(false),
                    Json::Bool(true)
                ),
            )
            .map_err(|e| e.to_string())?;
        let transactions = value
            .get("transactions")
            .and_then(Json::as_array)
            .ok_or("Missing transaction list")?;
        let mut visited = HashSet::new();
        for entry in transactions {
            if entry.get("category").and_then(Json::as_str) != Some("send") {
                continue;
            }
            let txid = entry
                .get("txid")
                .and_then(Json::as_str)
                .ok_or("Missing spender txid")?
                .parse::<bitcoin::Txid>()
                .map_err(|e| e.to_string())?;
            if txid == outpoint.txid || !visited.insert(txid) {
                continue;
            }
            let tx = self
                .try_get_transaction(&txid)?
                .ok_or("Spender disappeared during polling")?;
            if !tx
                .tx
                .input
                .iter()
                .any(|input| input.previous_output == *outpoint)
            {
                continue;
            }
            if tx.confirmations == 0
                && !tx.has_assumed_confirmation()
                && !tx.conflicting_txs.is_empty()
                && !self.try_is_in_mempool(&txid)?
            {
                continue;
            }
            return Ok(Some(txid));
        }
        Ok(None)
    }
}

impl BitcoinD {
    pub fn try_get_block_stats(&self, hash: bitcoin::BlockHash) -> Result<BlockStats, String> {
        let value = self
            .poll_node("getblockheader", params!(Json::String(hash.to_string())))
            .map_err(|e| e.to_string())?;
        let number = |key: &str| {
            value
                .get(key)
                .and_then(Json::as_i64)
                .ok_or_else(|| format!("Invalid block header field {key}"))
        };
        let previous_blockhash = value
            .get("previousblockhash")
            .map(|v| {
                v.as_str()
                    .ok_or_else(|| "Invalid previous block hash".to_string())?
                    .parse::<bitcoin::BlockHash>()
                    .map_err(|e| e.to_string())
            })
            .transpose()?;
        Ok(BlockStats {
            blockhash: hash,
            previous_blockhash,
            confirmations: number("confirmations")?
                .try_into()
                .map_err(|_| "Invalid confirmations")?,
            height: number("height")?.try_into().map_err(|_| "Invalid height")?,
            time: number("time")?.try_into().map_err(|_| "Invalid time")?,
            median_time_past: number("mediantime")?
                .try_into()
                .map_err(|_| "Invalid median time")?,
        })
    }

    pub fn try_tip_before_timestamp(
        &self,
        timestamp: u32,
    ) -> Result<Option<BlockChainTip>, String> {
        let error = std::cell::RefCell::new(None);
        let result = block_before_date(
            timestamp,
            self.try_chain_tip()?,
            |height| match self.try_get_block_hash(height) {
                Ok(hash) => hash,
                Err(e) => {
                    *error.borrow_mut() = Some(e);
                    None
                }
            },
            |hash| match self.try_get_block_stats(hash) {
                Ok(stats) => Some(stats),
                Err(e) => {
                    *error.borrow_mut() = Some(e);
                    None
                }
            },
        );
        match error.into_inner() {
            Some(error) => Err(error),
            None => Ok(result),
        }
    }
}

#[cfg(test)]
mod tests;

use std::convert::TryInto;

use bdk_electrum::bdk_chain::{bitcoin, BlockId, ConfirmationTimeHeightAnchor};

use crate::bitcoin::{BlockChainTip, BlockInfo};

pub fn height_u32_from_i32(height: i32) -> u32 {
    height.try_into().expect("height must fit into u32")
}

pub fn height_i32_from_u32(height: u32) -> i32 {
    height.try_into().expect("height must fit into i32")
}

pub fn height_i32_from_usize(height: usize) -> i32 {
    height.try_into().expect("height must fit into i32")
}

pub fn height_usize_from_i32(height: i32) -> usize {
    height.try_into().expect("height must fit into usize")
}

pub fn block_id_from_tip(tip: BlockChainTip) -> BlockId {
    BlockId {
        height: height_u32_from_i32(tip.height),
        hash: tip.hash,
    }
}

pub fn tip_from_block_id(id: BlockId) -> BlockChainTip {
    BlockChainTip {
        height: height_i32_from_u32(id.height),
        hash: id.hash,
    }
}

pub fn block_info_from_anchor(anchor: ConfirmationTimeHeightAnchor) -> BlockInfo {
    BlockInfo {
        height: height_i32_from_u32(anchor.confirmation_height),
        time: anchor
            .confirmation_time
            .try_into()
            .expect("u32 by consensus"),
    }
}

/// Get the transaction's outpoints.
pub fn outpoints_from_tx(tx: &bitcoin::Transaction) -> Vec<bitcoin::OutPoint> {
    let txid = tx.compute_txid();
    (0..tx.output.len())
        .map(|i| {
            bitcoin::OutPoint::new(txid, i.try_into().expect("num tx outputs must fit in u32"))
        })
        .collect::<Vec<_>>()
}

use std::convert::TryInto;

use bdk_electrum::bdk_chain::{
    bitcoin, local_chain::LocalChain, BlockId, ChainPosition, ConfirmationTimeHeightAnchor, TxGraph,
};

use crate::bitcoin::{BlockChainTip, BlockInfo, MempoolEntry, MempoolEntryFees};

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

pub fn height_usize_from_u32(height: u32) -> usize {
    height.try_into().expect("height must fit into usize")
}

pub fn block_id_from_tip(tip: BlockChainTip) -> BlockId {
    BlockId {
        height: height_u32_from_i32(tip.height),
        hash: tip.hash,
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

// FIXME: need to get ancestors & descendants
pub fn mempool_entry_from_graph(
    graph: &TxGraph<ConfirmationTimeHeightAnchor>,
    local_chain: &LocalChain,
    txid: &bitcoin::Txid,
) -> Option<MempoolEntry> {
    // Return an entry only if the tx is unconfirmed.
    let entry = if let Some(ChainPosition::Unconfirmed(_)) =
        graph.get_chain_position(local_chain, local_chain.tip().block_id(), *txid)
    {
        graph.get_tx(*txid).map(|tx| {
            let vsize: u64 = tx.vsize().try_into().expect("vsize must fit in u64");
            let fee = bitcoin::Amount::from_sat(
                graph.calculate_fee(&tx).expect("we have all prev txouts"),
            );
            let fees = MempoolEntryFees {
                base: fee,
                ancestor: fee,
                descendant: fee,
            };
            MempoolEntry {
                vsize,
                ancestor_vsize: vsize,
                fees,
            }
        })
    } else {
        None
    };
    entry
}

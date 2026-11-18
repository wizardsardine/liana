//! Test-only public wallets of every supported imported shape, with two
//! pre-fork coins usable as signing fixtures (`split_hardware`).

use std::str::FromStr;

use coincube_core::miniscript::bitcoin::{
    self, absolute,
    bip32::{DerivationPath, Xpriv, Xpub},
    hashes::Hash,
    secp256k1::Secp256k1,
    transaction, Amount, BlockHash, Network, OutPoint, Transaction, TxIn, TxOut, Txid,
};

use super::foreign_scan::{Branch, DiscoveredCoin, ScanDescriptor};

pub const BITCOIN_TIP_HEIGHT: u32 = 960;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    Wpkh,
    ShWpkh,
    Pkh,
    WshSortedMulti,
    WshMulti,
}

pub const SHAPES: [Shape; 5] = [
    Shape::Wpkh,
    Shape::ShWpkh,
    Shape::Pkh,
    Shape::WshSortedMulti,
    Shape::WshMulti,
];

pub struct Wallet {
    pub external: ScanDescriptor,
    pub internal: ScanDescriptor,
}

pub fn master(seed: u8) -> Xpriv {
    Xpriv::new_master(Network::Bitcoin, &[seed; 32]).unwrap()
}

fn account(master: &Xpriv, path: &str) -> String {
    let secp = Secp256k1::new();
    let child = master
        .derive_priv(&secp, &DerivationPath::from_str(path).unwrap())
        .unwrap();
    format!(
        "[{}/{}]{}",
        master.fingerprint(&secp),
        path.trim_start_matches("m/"),
        Xpub::from_priv(&secp, &child)
    )
}

/// The shape's descriptor with `{b}` in place of the branch step.
fn template(shape: Shape) -> String {
    match shape {
        Shape::Wpkh => format!("wpkh({}/{{b}}/*)", account(&master(1), "m/84'/0'/0'")),
        Shape::ShWpkh => format!("sh(wpkh({}/{{b}}/*))", account(&master(1), "m/49'/0'/0'")),
        Shape::Pkh => format!("pkh({}/{{b}}/*)", account(&master(1), "m/44'/0'/0'")),
        Shape::WshSortedMulti | Shape::WshMulti => {
            let keys: Vec<_> = [1, 2, 3]
                .iter()
                .map(|seed| format!("{}/{{b}}/*", account(&master(*seed), "m/48'/0'/0'/2'")))
                .collect();
            let name = if shape == Shape::WshMulti {
                "multi"
            } else {
                "sortedmulti"
            };
            format!("wsh({name}(2,{}))", keys.join(","))
        }
    }
}

pub fn wallet(shape: Shape) -> Wallet {
    let template = template(shape);
    let parse = |branch, step: u32| {
        ScanDescriptor::parse(branch, &template.replace("{b}", &step.to_string())).unwrap()
    };
    Wallet {
        external: parse(Branch::External, 0),
        internal: parse(Branch::Internal, 1),
    }
}

/// The shape as an imported Bitcoin Vault, the way the app loads it.
pub fn imported_vault(shape: Shape) -> crate::app::wallet::Wallet {
    let template = template(shape);
    let branch = |step: u32| template.replace("{b}", &step.to_string());
    let descriptor = coincube_core::wallet_descriptor::ImportedWalletDescriptor::parse(
        &branch(0),
        Some(&branch(1)),
        crate::chain::ChainId::Bitcoin,
    )
    .unwrap();
    crate::app::wallet::Wallet::new(
        coincube_core::wallet_descriptor::WalletDescriptor::Imported(descriptor),
    )
}

/// The same wallet as [`imported_vault`], imported on Bitcoin Blake2b.
pub fn imported_fork_vault(shape: Shape) -> crate::app::wallet::Wallet {
    let bitcoin = imported_vault(shape).main_descriptor;
    crate::app::wallet::Wallet::new(
        coincube_core::wallet_descriptor::WalletDescriptor::Imported(
            bitcoin.as_imported().unwrap().paired_blake2b().unwrap(),
        ),
    )
}

pub fn block_hash(height: u64) -> BlockHash {
    let mut bytes = [0; 32];
    bytes[..8].copy_from_slice(&height.to_le_bytes());
    BlockHash::from_byte_array(bytes)
}

/// A coin paying `wallet`'s address at `branch`/`index`, confirmed at
/// `height` (unconfirmed when `None`).
pub fn coin(
    wallet: &Wallet,
    branch: Branch,
    index: u32,
    sats: u64,
    height: Option<u32>,
) -> DiscoveredCoin {
    let descriptor = match branch {
        Branch::External => &wallet.external,
        Branch::Internal => &wallet.internal,
    };
    let previous = Transaction {
        version: transaction::Version::TWO,
        lock_time: absolute::LockTime::ZERO,
        input: vec![TxIn {
            previous_output: OutPoint::new(
                Txid::from_byte_array([index as u8 + 1; 32]),
                sats as u32,
            ),
            ..TxIn::default()
        }],
        output: vec![
            TxOut {
                value: Amount::from_sat(1_000),
                script_pubkey: bitcoin::ScriptBuf::new_op_return([index as u8]),
            },
            TxOut {
                value: Amount::from_sat(sats),
                script_pubkey: descriptor.script(index).unwrap(),
            },
        ],
    };
    DiscoveredCoin {
        branch,
        index,
        outpoint: OutPoint::new(previous.compute_txid(), 1),
        output: previous.output[1].clone(),
        previous,
        confirmed: height.is_some(),
        block_height: height,
        block_hash: height.map(|h| block_hash(u64::from(h))),
    }
}

/// The two pre-fork coins of `wallet` on both chains, joined.
pub fn shared_coins(wallet: &Wallet) -> Vec<DiscoveredCoin> {
    vec![
        coin(wallet, Branch::External, 0, 100_000, Some(880)),
        coin(wallet, Branch::Internal, 3, 50_000, Some(890)),
    ]
}

/// An unused receive index of every fixture wallet: the walk's last used
/// index is 3 on both branches.
pub const FRESH_RECEIVE: u32 = 4;

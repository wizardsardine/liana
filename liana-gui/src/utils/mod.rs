use liana::miniscript::bitcoin::{self, Network};

#[cfg(test)]
pub mod sandbox;

#[cfg(test)]
pub mod mock;

pub fn example_xpub(network: Network) -> String {
    format!("[aabbccdd/42'/0']{}pub6DAkq8LWw91WGgUGnkR5Sbzjev5JCsXaTVZQ9MwsPV4BkNFKygtJ8GHodfDVx1udR723nT7JASqGPpKvz7zQ25pUTW6zVEBdiWoaC4aUqik",
        if network == bitcoin::Network::Bitcoin { "x" } else { "t" }
    )
}

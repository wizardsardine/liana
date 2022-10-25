use std::convert::TryFrom;

use minisafe::config::Config as MinisafeConfig;

use super::step::Context;

pub const DEFAULT_FILE_NAME: &str = "daemon.toml";

impl TryFrom<Context> for MinisafeConfig {
    type Error = &'static str;

    fn try_from(ctx: Context) -> Result<Self, Self::Error> {
        if ctx.descriptor.is_none() {
            return Err("config does not have a main Descriptor");
        }
        Ok(MinisafeConfig {
            #[cfg(unix)]
            daemon: false,
            log_level: log::LevelFilter::Info,
            main_descriptor: ctx.descriptor.unwrap(),
            data_dir: ctx.data_dir,
            bitcoin_config: ctx.bitcoin_config,
            bitcoind_config: ctx.bitcoind_config,
        })
    }
}

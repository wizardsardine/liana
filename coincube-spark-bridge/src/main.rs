//! Sibling process that hosts the Breez Spark SDK and exposes it to
//! `coincube-gui` over a stdin/stdout JSON-RPC stream.
//!
//! Why a separate process: the Liquid and Spark SDKs can't coexist in a
//! single Rust binary today (incompatible `rusqlite`/`libsqlite3-sys`
//! graphs, conflicting `links = "sqlite3"` attribute). The gui links
//! Liquid; this bridge links Spark; they talk via line-delimited JSON.
//!
//! Modes of operation:
//! - Default: run as a subprocess speaking [`coincube_spark_protocol::Frame`]
//!   messages on stdin/stdout. Parent process drives the lifecycle.
//! - `--smoke-test`: connect to Spark mainnet using env vars
//!   (`BREEZ_API_KEY`, `COINCUBE_SPARK_MNEMONIC`,
//!   `COINCUBE_SPARK_STORAGE_DIR`), fetch info + list payments, print to
//!   stdout, and exit. Intended as the Phase 2 standalone harness.

mod sdk_adapter;
mod server;

use clap::Parser;
use tracing_subscriber::EnvFilter;

/// CLI options for the bridge binary.
#[derive(Debug, Parser)]
#[command(
    name = "coincube-spark-bridge",
    about = "Sibling process hosting breez-sdk-spark for coincube-gui"
)]
struct Cli {
    /// Run a one-shot connect + info + list-payments round trip against
    /// mainnet using env configuration, print the result, and exit.
    ///
    /// Required env vars: `BREEZ_API_KEY`, `COINCUBE_SPARK_MNEMONIC`,
    /// `COINCUBE_SPARK_STORAGE_DIR`.
    #[arg(long)]
    smoke_test: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Send all tracing output to stderr so the stdout channel stays clean
    // for JSON frames.
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,breez_sdk_spark=warn")),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    if cli.smoke_test {
        smoke_test::run().await
    } else {
        server::run().await
    }
}

mod smoke_test {
    //! Standalone harness. Not used when the bridge runs under a parent process.

    use std::env;

    use breez_sdk_spark::{GetInfoRequest, ListPaymentsRequest};

    use crate::sdk_adapter::{self, SdkHandle};

    pub async fn run() -> anyhow::Result<()> {
        let api_key = env::var("BREEZ_API_KEY")
            .map_err(|_| anyhow::anyhow!("BREEZ_API_KEY must be set for --smoke-test"))?;
        let mnemonic = env::var("COINCUBE_SPARK_MNEMONIC").map_err(|_| {
            anyhow::anyhow!("COINCUBE_SPARK_MNEMONIC must be set for --smoke-test")
        })?;
        let storage_dir = env::var("COINCUBE_SPARK_STORAGE_DIR").map_err(|_| {
            anyhow::anyhow!("COINCUBE_SPARK_STORAGE_DIR must be set for --smoke-test")
        })?;

        eprintln!("connecting to Spark mainnet with storage at {storage_dir}");
        let handle: SdkHandle = sdk_adapter::connect_mainnet(
            api_key,
            mnemonic,
            None, // passphrase
            storage_dir,
        )
        .await?;

        let info = handle
            .sdk
            .get_info(GetInfoRequest {
                ensure_synced: Some(true),
            })
            .await?;
        eprintln!(
            "connected — identity {}, balance {} sats",
            info.identity_pubkey, info.balance_sats
        );
        println!("{}", serde_json::to_string_pretty(&info)?);

        let payments = handle
            .sdk
            .list_payments(ListPaymentsRequest {
                limit: Some(20),
                offset: Some(0),
                sort_ascending: Some(false),
                type_filter: None,
                status_filter: None,
                asset_filter: None,
                payment_details_filter: None,
                from_timestamp: None,
                to_timestamp: None,
            })
            .await?;
        eprintln!("fetched {} recent payments", payments.payments.len());
        println!("{}", serde_json::to_string_pretty(&payments.payments)?);

        Ok(())
    }
}

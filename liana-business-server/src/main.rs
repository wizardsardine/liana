use clap::Parser;
use std::error::Error;

mod auth;
mod connection;
mod handler;
mod http;
mod server;
mod state;

#[cfg(test)]
mod tests;

use server::Server;

#[derive(Parser, Debug)]
#[command(name = "liana-business-server")]
#[command(about = "Liana Business WebSocket Server", long_about = None)]
#[command(version)]
struct Args {
    /// Host address to bind to
    #[arg(long, default_value = "0.0.0.0")]
    host: String,

    /// HTTP port for auth endpoints (REST API)
    #[arg(long, default_value = "8080")]
    auth: u16,

    /// WebSocket port
    #[arg(long, default_value = "8081")]
    ws: u16,

    /// Log level (error, warn, info, debug, trace)
    #[arg(long, default_value = "info")]
    log_level: String,
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();

    // Initialize logger
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(&args.log_level))
        .init();

    log::info!("Starting Liana Business Server");
    log::info!("Auth API on {}:{}", args.host, args.auth);
    log::info!("WebSocket on {}:{}", args.host, args.ws);

    // Create and start server
    let mut server = Server::new(&args.host, args.auth, args.ws)?;
    server.print_tokens();
    server.run()?;

    Ok(())
}

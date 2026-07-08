#![allow(dead_code)]

mod config;
mod control;
mod metrics;
mod pacing;
mod room;
mod router;
mod session;
mod transport;

use clap::Parser;
use tracing::info;

use crate::{config::ServerConfig, transport::build_server_endpoint};

#[derive(Debug, Parser)]
#[command(author, version, about = "TeamView QUIC Relay/SFU server")]
struct Args {
    #[arg(long, default_value = "0.0.0.0:4433")]
    listen: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let args = Args::parse();
    let config = ServerConfig::new(args.listen);
    let endpoint = build_server_endpoint(&config.listen_addr)?;
    let local_addr = endpoint.local_addr()?;

    info!(listen = %local_addr, "relay server QUIC endpoint ready");
    println!("relay-server listening on {local_addr}");

    endpoint.wait_idle().await;
    Ok(())
}

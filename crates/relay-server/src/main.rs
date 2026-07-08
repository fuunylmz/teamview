#![allow(dead_code)]

mod config;
mod metrics;
mod pacing;
mod room;
mod router;
mod session;
mod transport;

use clap::Parser;
use tracing::info;

use crate::config::ServerConfig;

#[derive(Debug, Parser)]
#[command(author, version, about = "TeamView QUIC Relay/SFU server")]
struct Args {
    #[arg(long, default_value = "0.0.0.0:4433")]
    listen: String,
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let args = Args::parse();
    let config = ServerConfig::new(args.listen);

    info!(listen = %config.listen_addr, "relay server scaffold ready");
    println!(
        "relay-server scaffold listening target: {}",
        config.listen_addr
    );

    Ok(())
}

#![allow(dead_code)]

mod app;
mod capture;
mod decode;
mod encode;
mod playback;
mod stats;
mod transport;

use clap::{Parser, ValueEnum};
use tracing::info;

use crate::transport::quic::build_client_endpoint;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Mode {
    Broadcaster,
    Viewer,
}

#[derive(Debug, Parser)]
#[command(author, version, about = "TeamView native desktop client scaffold")]
struct Args {
    #[arg(long, value_enum, default_value_t = Mode::Viewer)]
    mode: Mode,

    #[arg(long, default_value = "127.0.0.1:4433")]
    relay: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let args = Args::parse();
    let endpoint = build_client_endpoint("127.0.0.1:0")?;
    let local_addr = endpoint.local_addr()?;

    info!(?args.mode, relay = %args.relay, local = %local_addr, "desktop client QUIC endpoint ready");
    println!(
        "desktop-client mode={:?} relay={} local={}",
        args.mode, args.relay, local_addr
    );

    Ok(())
}

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

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let args = Args::parse();
    info!(?args.mode, relay = %args.relay, "desktop client scaffold ready");
    println!(
        "desktop-client scaffold mode={:?} relay={}",
        args.mode, args.relay
    );

    Ok(())
}

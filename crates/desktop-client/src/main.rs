#![allow(dead_code)]

mod app;
mod capture;
mod decode;
mod encode;
mod playback;
mod stats;
mod transport;

use clap::{Parser, ValueEnum};
use teamview_protocol::{
    PROTOCOL_VERSION,
    control::{ClientControl, ClientEnvelope, Hello},
};
use tracing::info;

use crate::{
    capture::windows,
    transport::quic::{build_client_endpoint, send_control_request},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Mode {
    Broadcaster,
    Viewer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum CaptureSourceArg {
    PrimaryMonitor,
}

#[derive(Debug, Parser)]
#[command(author, version, about = "TeamView native desktop client scaffold")]
struct Args {
    #[arg(long, value_enum, default_value_t = Mode::Viewer)]
    mode: Mode,

    #[arg(long, default_value = "127.0.0.1:4433")]
    relay: String,

    #[arg(long, value_enum, default_value_t = CaptureSourceArg::PrimaryMonitor)]
    capture_source: CaptureSourceArg,

    #[arg(long, default_value_t = true)]
    cursor_visible: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let args = Args::parse();
    let endpoint = build_client_endpoint("127.0.0.1:0")?;
    let local_addr = endpoint.local_addr()?;
    let capture_supported = windows::is_supported();

    info!(
        ?args.mode,
        relay = %args.relay,
        local = %local_addr,
        capture_supported,
        ?args.capture_source,
        cursor_visible = args.cursor_visible,
        "desktop client endpoint and capture foundation ready"
    );
    println!(
        "desktop-client mode={:?} relay={} local={} capture_supported={} capture_source={:?}",
        args.mode, args.relay, local_addr, capture_supported, args.capture_source
    );

    let hello = ClientEnvelope::new(
        1,
        ClientControl::Hello(Hello {
            protocol_version: PROTOCOL_VERSION,
            client_name: format!("desktop-client/{:?}", args.mode),
        }),
    );
    let response = send_control_request(&endpoint, &args.relay, &hello).await?;
    println!(
        "control-response request_id={} message={:?}",
        response.request_id, response.message
    );

    Ok(())
}

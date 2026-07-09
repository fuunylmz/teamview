use clap::Parser;
use relay_server::{
    config::ServerConfig,
    control::{
        ControlLimits, DEFAULT_MAX_PARTICIPANTS_PER_ROOM, DEFAULT_MAX_ROOMS,
        DEFAULT_MAX_STREAMS_PER_ROOM,
    },
    control_stream::serve_control_endpoint,
    transport::build_server_endpoint,
};
use tracing::info;

#[derive(Debug, Parser)]
#[command(author, version, about = "TeamView QUIC Relay/SFU server")]
struct Args {
    #[arg(long, default_value = "0.0.0.0:4433")]
    listen: String,

    #[arg(long)]
    access_token: Option<String>,

    #[arg(long, default_value_t = 100)]
    viewer_queue_budget_ms: u16,

    #[arg(long, default_value_t = teamview_protocol::packet::DEFAULT_DATAGRAM_PAYLOAD_TARGET)]
    max_datagram_payload: usize,

    #[arg(long, default_value_t = DEFAULT_MAX_ROOMS)]
    max_rooms: usize,

    #[arg(long, default_value_t = DEFAULT_MAX_PARTICIPANTS_PER_ROOM)]
    max_participants_per_room: usize,

    #[arg(long, default_value_t = DEFAULT_MAX_STREAMS_PER_ROOM)]
    max_streams_per_room: usize,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let args = Args::parse();
    let mut config = ServerConfig::new(args.listen)
        .with_access_token(args.access_token)
        .with_max_datagram_payload(args.max_datagram_payload)
        .with_control_limits(ControlLimits {
            max_rooms: args.max_rooms,
            max_participants_per_room: args.max_participants_per_room,
            max_streams_per_room: args.max_streams_per_room,
        });
    config.viewer_queue_budget_ms = args.viewer_queue_budget_ms.max(1);
    let endpoint = build_server_endpoint(&config.listen_addr)?;
    let local_addr = endpoint.local_addr()?;

    info!(
        listen = %local_addr,
        auth_required = config.access_token.is_some(),
        "relay server QUIC endpoint ready"
    );
    println!("relay-server listening on {local_addr}");

    serve_control_endpoint(endpoint, config).await;
    Ok(())
}

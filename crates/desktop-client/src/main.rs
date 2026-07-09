#![allow(dead_code)]

mod app;
mod capture;
mod decode;
mod encode;
mod playback;
mod stats;
mod transport;

use std::time::Duration;

use anyhow::{Context, bail};
use clap::{Parser, ValueEnum};
use teamview_protocol::{
    PROTOCOL_VERSION,
    codec::CodecId,
    control::{
        ClientControl, CreateRoom, Hello, JoinRoom, MediaKind, PublishStream, RoomId,
        ServerControl, ServerEnvelope, StreamId, SubscribeStream,
    },
    frame::packetize_frame_for_datagram_target,
    packet::DEFAULT_DATAGRAM_PAYLOAD_TARGET,
};
use tracing::info;

use crate::{
    capture::{CaptureConfig, CaptureSource, ScreenCapture, windows},
    decode::{FrameReassemblyBuffer, VideoDecoder, h264::H264Decoder},
    encode::{VideoEncoder, h264::H264Encoder},
    playback::{NullPlayback, VideoPlayback},
    transport::quic::{build_client_endpoint, connect_control_client},
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

    #[arg(long)]
    room_id: Option<RoomId>,

    #[arg(long, default_value = "stage1")]
    room_name: String,

    #[arg(long, default_value_t = 1)]
    stream_id: StreamId,

    #[arg(long, default_value_t = 0)]
    media_frames: u32,

    #[arg(long, default_value_t = 512)]
    media_frame_bytes: usize,

    #[arg(long, default_value_t = DEFAULT_DATAGRAM_PAYLOAD_TARGET)]
    max_datagram_payload: usize,

    #[arg(long, default_value_t = 0)]
    media_start_delay_ms: u64,
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

    let mut control = connect_control_client(&endpoint, &args.relay).await?;
    let response = control
        .send(ClientControl::Hello(Hello {
            protocol_version: PROTOCOL_VERSION,
            client_name: format!("desktop-client/{:?}", args.mode),
        }))
        .await?;
    print_control_response("hello", &response);

    match args.mode {
        Mode::Broadcaster => run_broadcaster_control_flow(&mut control, &args).await?,
        Mode::Viewer => run_viewer_control_flow(&mut control, &args).await?,
    }

    Ok(())
}

async fn run_broadcaster_control_flow(
    control: &mut crate::transport::quic::ControlClient,
    args: &Args,
) -> anyhow::Result<()> {
    let room_id = match args.room_id {
        Some(room_id) => room_id,
        None => {
            let response = control
                .send(ClientControl::CreateRoom(CreateRoom {
                    name: args.room_name.clone(),
                }))
                .await?;
            print_control_response("create-room", &response);
            match response.message {
                ServerControl::RoomCreated(room) => room.room_id,
                ServerControl::Error(error) => bail!("create room failed: {}", error.message),
                other => bail!("unexpected create room response: {other:?}"),
            }
        }
    };

    let joined = control
        .send(ClientControl::JoinRoom(JoinRoom { room_id }))
        .await?;
    print_control_response("join-room", &joined);
    ensure_not_error("join room", &joined)?;

    let published = control
        .send(ClientControl::PublishStream(PublishStream {
            room_id,
            stream_id: args.stream_id,
            codec: CodecId::H264,
            media_kind: MediaKind::Screen,
        }))
        .await?;
    print_control_response("publish-stream", &published);
    ensure_not_error("publish stream", &published)?;

    println!(
        "control-flow broadcaster room_id={} stream_id={}",
        room_id, args.stream_id
    );
    if args.media_frames > 0 {
        run_synthetic_broadcaster_media(control, args).await?;
    }
    Ok(())
}

async fn run_viewer_control_flow(
    control: &mut crate::transport::quic::ControlClient,
    args: &Args,
) -> anyhow::Result<()> {
    let room_id = args
        .room_id
        .context("viewer mode requires --room-id from a broadcaster run")?;

    let joined = control
        .send(ClientControl::JoinRoom(JoinRoom { room_id }))
        .await?;
    print_control_response("join-room", &joined);
    ensure_not_error("join room", &joined)?;

    let subscribed = control
        .send(ClientControl::SubscribeStream(SubscribeStream {
            room_id,
            stream_id: args.stream_id,
        }))
        .await?;
    print_control_response("subscribe-stream", &subscribed);
    ensure_not_error("subscribe stream", &subscribed)?;

    println!(
        "control-flow viewer room_id={} stream_id={}",
        room_id, args.stream_id
    );
    if args.media_frames > 0 {
        run_synthetic_viewer_media(control, args).await?;
    }
    Ok(())
}

async fn run_synthetic_broadcaster_media(
    control: &crate::transport::quic::ControlClient,
    args: &Args,
) -> anyhow::Result<()> {
    if args.media_start_delay_ms > 0 {
        tokio::time::sleep(Duration::from_millis(args.media_start_delay_ms)).await;
    }

    let mut capture = windows::WindowsGraphicsCapture::new(
        CaptureSource::PrimaryMonitor,
        CaptureConfig {
            queue_capacity: 1,
            cursor_visible: args.cursor_visible,
        },
    )?;
    let mut encoder = H264Encoder::default();
    encoder.config.synthetic_payload_bytes = args.media_frame_bytes;
    encoder.request_keyframe();

    let mut sent_packets = 0_u64;
    for frame_id in 1..=args.media_frames {
        capture.push_test_frame(1280, 720, frame_id as u64 * 33_333);
        let Some(captured) = capture.next_frame()? else {
            continue;
        };
        let Some(frame) = encoder.encode(captured, args.stream_id)? else {
            continue;
        };
        let packets = packetize_frame_for_datagram_target(
            &frame,
            frame_id.saturating_mul(100),
            args.max_datagram_payload,
        )?;
        for packet in &packets {
            control.send_media_packet(packet)?;
            sent_packets += 1;
        }
        println!(
            "media-send frame_id={} fragments={} bytes={} target_bytes={}",
            frame.frame_id,
            packets.len(),
            frame.bytes.len(),
            args.media_frame_bytes
        );
    }
    println!(
        "media-summary role=broadcaster frames={} packets={}",
        args.media_frames, sent_packets
    );
    Ok(())
}

async fn run_synthetic_viewer_media(
    control: &crate::transport::quic::ControlClient,
    args: &Args,
) -> anyhow::Result<()> {
    let mut buffer = FrameReassemblyBuffer::new();
    let mut decoder = H264Decoder;
    let mut playback = NullPlayback;
    let mut reassembled_frames = 0_u32;
    let mut decoded_frames = 0_u32;
    let mut received_packets = 0_u64;

    while reassembled_frames < args.media_frames {
        let packet = tokio::time::timeout(Duration::from_secs(5), control.recv_media_packet())
            .await
            .context("timed out waiting for media packet")??;
        received_packets += 1;
        if let Some(frame) = buffer.push(packet)? {
            println!(
                "media-recv frame_id={} bytes={} keyframe={}",
                frame.frame_id,
                frame.bytes.len(),
                frame.is_keyframe
            );
            if let Some(decoded) = decoder.decode(&frame.bytes)? {
                playback.render(decoded)?;
                decoded_frames += 1;
            }
            reassembled_frames += 1;
        }
    }

    println!(
        "media-summary role=viewer frames={} decoded={} packets={}",
        reassembled_frames, decoded_frames, received_packets
    );
    Ok(())
}

fn print_control_response(stage: &str, response: &ServerEnvelope) {
    println!(
        "control-response stage={} request_id={} message={:?}",
        stage, response.request_id, response.message
    );
}

fn ensure_not_error(action: &str, response: &ServerEnvelope) -> anyhow::Result<()> {
    if let ServerControl::Error(error) = &response.message {
        bail!("{action} failed: {}", error.message);
    }
    Ok(())
}

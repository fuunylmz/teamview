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
        ServerControl, ServerEnvelope, StreamId, SubscribeStream, ViewerStatsReport,
    },
    frame::packetize_frame_for_datagram_target,
    packet::DEFAULT_DATAGRAM_PAYLOAD_TARGET,
};
use tokio::time::MissedTickBehavior;
use tracing::info;

use crate::{
    capture::{CaptureConfig, CaptureSource, ScreenCapture, windows},
    decode::{FrameReassemblyBuffer, VideoDecoder, h264::H264Decoder},
    encode::{VideoEncoder, h264::H264Encoder},
    playback::{NullPlayback, VideoPlayback},
    stats::ClientMediaStats,
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

    #[arg(long, default_value_t = 0)]
    media_run_ms: u64,

    #[arg(long, default_value_t = 30)]
    media_fps: u16,

    #[arg(long, default_value_t = 512)]
    media_frame_bytes: usize,

    #[arg(long, default_value_t = DEFAULT_DATAGRAM_PAYLOAD_TARGET)]
    max_datagram_payload: usize,

    #[arg(long, default_value_t = 0)]
    media_start_delay_ms: u64,

    #[arg(long, default_value_t = 250)]
    media_end_linger_ms: u64,

    #[arg(long, default_value_t = 5_000)]
    media_idle_timeout_ms: u64,

    #[arg(long, default_value_t = 30)]
    stats_interval_frames: u32,
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
    if args.synthetic_media_enabled() {
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
    if args.synthetic_media_enabled() {
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
    let media_frames = args.synthetic_media_frames()?;
    let frame_interval = args.media_frame_interval()?;

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

    let mut ticker = tokio::time::interval(frame_interval);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let capture_step_micros = frame_interval.as_micros().min(u64::MAX as u128) as u64;
    let mut sent_packets = 0_u64;
    let mut next_sequence_number = 1_u32;
    for frame_id in 1..=media_frames {
        ticker.tick().await;
        capture.push_test_frame(1280, 720, frame_id as u64 * capture_step_micros.max(1));
        let Some(captured) = capture.next_frame()? else {
            continue;
        };
        let Some(frame) = encoder.encode(captured, args.stream_id)? else {
            continue;
        };
        let packets = packetize_frame_for_datagram_target(
            &frame,
            next_sequence_number,
            args.max_datagram_payload,
        )?;
        next_sequence_number = next_sequence_number.wrapping_add(packets.len() as u32);
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
        "media-summary role=broadcaster frames={} packets={} fps={} run_ms={}",
        media_frames, sent_packets, args.media_fps, args.media_run_ms
    );
    if args.media_end_linger_ms > 0 {
        tokio::time::sleep(Duration::from_millis(args.media_end_linger_ms)).await;
    }
    Ok(())
}

async fn run_synthetic_viewer_media(
    control: &mut crate::transport::quic::ControlClient,
    args: &Args,
) -> anyhow::Result<()> {
    let target_frames = args.synthetic_media_frames()?;
    let mut buffer = FrameReassemblyBuffer::new();
    let mut decoder = H264Decoder;
    let mut playback = NullPlayback;
    let mut stats = ClientMediaStats::default();
    let mut reassembled_frames = 0_u32;
    let mut decoded_frames = 0_u32;
    let mut received_packets = 0_u64;

    while reassembled_frames < target_frames {
        let packet = match tokio::time::timeout(
            Duration::from_millis(args.media_idle_timeout_ms),
            control.recv_media_packet(),
        )
        .await
        {
            Ok(Ok(packet)) => packet,
            Ok(Err(error)) => return Err(error),
            Err(error) => {
                if args.media_frames == 0 && received_packets > 0 {
                    break;
                }
                return Err(error).context("timed out waiting for media packet");
            }
        };
        stats.record_packet(&packet);
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
                stats.record_decoded_frame();
                decoded_frames += 1;
            } else {
                stats.record_dropped_frame();
            }
            reassembled_frames += 1;
            if args.stats_interval_frames > 0
                && reassembled_frames.is_multiple_of(args.stats_interval_frames)
            {
                send_viewer_stats(control, args.room_id.unwrap(), args.stream_id, stats).await?;
            }
        }
    }
    if received_packets > 0 {
        send_viewer_stats(control, args.room_id.unwrap(), args.stream_id, stats).await?;
    }

    println!(
        "media-summary role=viewer frames={} decoded={} packets={} lost={} dropped={}",
        reassembled_frames,
        decoded_frames,
        received_packets,
        stats.lost_packets,
        stats.dropped_frames
    );
    Ok(())
}

async fn send_viewer_stats(
    control: &mut crate::transport::quic::ControlClient,
    room_id: RoomId,
    stream_id: StreamId,
    stats: ClientMediaStats,
) -> anyhow::Result<()> {
    let report: ViewerStatsReport = stats.to_viewer_report(room_id, stream_id);
    let response = control.send(ClientControl::ViewerStats(report)).await?;
    print_control_response("viewer-stats", &response);
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

impl Args {
    fn synthetic_media_enabled(&self) -> bool {
        self.media_frames > 0 || self.media_run_ms > 0
    }

    fn synthetic_media_frames(&self) -> anyhow::Result<u32> {
        if self.media_frames > 0 {
            return Ok(self.media_frames);
        }
        if self.media_fps == 0 {
            bail!("--media-fps must be greater than zero");
        }
        let frames = self
            .media_run_ms
            .saturating_mul(self.media_fps as u64)
            .div_ceil(1_000)
            .max(1);
        Ok(frames.min(u32::MAX as u64) as u32)
    }

    fn media_frame_interval(&self) -> anyhow::Result<Duration> {
        if self.media_fps == 0 {
            bail!("--media-fps must be greater than zero");
        }
        Ok(Duration::from_micros(1_000_000 / self.media_fps as u64))
    }
}

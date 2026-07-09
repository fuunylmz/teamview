#![allow(dead_code)]

mod app;
mod capture;
mod decode;
mod encode;
mod playback;
mod stats;
mod transport;

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, bail};
use clap::{Parser, ValueEnum};
use teamview_protocol::{
    PROTOCOL_VERSION,
    codec::CodecId,
    control::{
        ClientControl, CreateRoom, Hello, JoinRoom, KeyframeReason, MediaKind,
        PollPublisherFeedback, PollStreamConfig, PublishStream, PublisherFeedback, RequestKeyframe,
        RoomId, ServerControl, ServerEnvelope, SetTargetBitrate, SetTargetFramerate, StreamConfig,
        StreamId, SubscribeStream, ViewerStatsReport,
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

const MIN_SYNTHETIC_PAYLOAD_BYTES: usize = 64;

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

    #[arg(long, default_value_t = 6)]
    reassembly_window_frames: u32,

    #[arg(long, default_value_t = 30)]
    stats_interval_frames: u32,

    #[arg(long, default_value_t = 30)]
    feedback_interval_frames: u32,
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

    let stream_config = args.synthetic_stream_config(room_id);
    let configured = control
        .send(ClientControl::SetStreamConfig(stream_config.clone()))
        .await?;
    print_control_response("set-stream-config", &configured);
    match configured.message {
        ServerControl::StreamConfig(config) if config == stream_config => {}
        ServerControl::Error(error) => bail!("set stream config failed: {}", error.message),
        other => bail!("unexpected stream config response: {other:?}"),
    }

    println!(
        "control-flow broadcaster room_id={} stream_id={}",
        room_id, args.stream_id
    );
    if args.synthetic_media_enabled() {
        set_publisher_target_media(control, room_id, args).await?;
        run_synthetic_broadcaster_media(control, args, room_id).await?;
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

    let stream_config = poll_stream_config(control, room_id, args.stream_id).await?;
    println!(
        "stream-config stream_id={} codec={:?} width={} height={} fps={} timebase_hz={}",
        stream_config.stream_id,
        stream_config.codec,
        stream_config.width,
        stream_config.height,
        stream_config.frames_per_second,
        stream_config.timebase_hz
    );

    println!(
        "control-flow viewer room_id={} stream_id={}",
        room_id, args.stream_id
    );
    if args.synthetic_media_enabled() {
        run_synthetic_viewer_media(control, args).await?;
    }
    Ok(())
}

async fn poll_stream_config(
    control: &mut crate::transport::quic::ControlClient,
    room_id: RoomId,
    stream_id: StreamId,
) -> anyhow::Result<StreamConfig> {
    let response = control
        .send(ClientControl::PollStreamConfig(PollStreamConfig {
            room_id,
            stream_id,
        }))
        .await?;
    print_control_response("poll-stream-config", &response);
    match response.message {
        ServerControl::StreamConfig(config) => Ok(config),
        ServerControl::Error(error) => bail!("poll stream config failed: {}", error.message),
        other => bail!("unexpected stream config response: {other:?}"),
    }
}

async fn run_synthetic_broadcaster_media(
    control: &mut crate::transport::quic::ControlClient,
    args: &Args,
    room_id: RoomId,
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
    encoder.config.bitrate_bps = args.synthetic_bitrate_bps();
    encoder.config.frames_per_second = args.media_fps;
    encoder.request_keyframe();

    let mut ticker = tokio::time::interval(frame_interval);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut active_fps = args.media_fps;
    let mut sent_packets = 0_u64;
    let mut next_sequence_number = 1_u32;
    for frame_id in 1..=media_frames {
        ticker.tick().await;
        capture.push_test_frame(1280, 720, unix_time_micros());
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
        if args.feedback_interval_frames > 0
            && frame_id.is_multiple_of(args.feedback_interval_frames)
        {
            let feedback = poll_publisher_feedback(control, room_id, args.stream_id).await?;
            if feedback.keyframe_requested {
                encoder.request_keyframe();
            }
            apply_publisher_feedback(&feedback, &mut encoder, &mut active_fps, &mut ticker);
        }
    }
    let feedback = poll_publisher_feedback(control, room_id, args.stream_id).await?;
    if feedback.keyframe_requested {
        println!(
            "publisher-feedback stream_id={} keyframe_requested=true degraded_viewers={} total_viewers={}",
            feedback.stream_id, feedback.degraded_viewer_count, feedback.total_viewer_count
        );
    }
    apply_publisher_feedback(&feedback, &mut encoder, &mut active_fps, &mut ticker);
    println!(
        "media-summary role=broadcaster frames={} packets={} fps={} run_ms={}",
        media_frames, sent_packets, args.media_fps, args.media_run_ms
    );
    if args.media_end_linger_ms > 0 {
        tokio::time::sleep(Duration::from_millis(args.media_end_linger_ms)).await;
    }
    Ok(())
}

async fn set_publisher_target_media(
    control: &mut crate::transport::quic::ControlClient,
    room_id: RoomId,
    args: &Args,
) -> anyhow::Result<()> {
    let bitrate = control
        .send(ClientControl::SetTargetBitrate(SetTargetBitrate {
            room_id,
            stream_id: args.stream_id,
            bitrate_bps: args.synthetic_bitrate_bps(),
        }))
        .await?;
    print_control_response("set-target-bitrate", &bitrate);
    ensure_not_error("set target bitrate", &bitrate)?;

    let framerate = control
        .send(ClientControl::SetTargetFramerate(SetTargetFramerate {
            room_id,
            stream_id: args.stream_id,
            frames_per_second: args.media_fps.max(1),
        }))
        .await?;
    print_control_response("set-target-framerate", &framerate);
    ensure_not_error("set target framerate", &framerate)?;
    Ok(())
}

fn apply_publisher_feedback(
    feedback: &PublisherFeedback,
    encoder: &mut H264Encoder,
    active_fps: &mut u16,
    ticker: &mut tokio::time::Interval,
) {
    let mut adapted = false;
    let target_bitrate = feedback.aggregate_available_bitrate_bps;
    if target_bitrate > 0 && target_bitrate != encoder.config.bitrate_bps {
        encoder.update_bitrate(target_bitrate);
        encoder.config.synthetic_payload_bytes =
            synthetic_payload_bytes_for_bitrate(target_bitrate, (*active_fps).max(1));
        adapted = true;
    }
    if feedback.target_frames_per_second > 0 && feedback.target_frames_per_second != *active_fps {
        *active_fps = feedback.target_frames_per_second;
        encoder.config.frames_per_second = *active_fps;
        encoder.config.synthetic_payload_bytes =
            synthetic_payload_bytes_for_bitrate(encoder.config.bitrate_bps, (*active_fps).max(1));
        *ticker = tokio::time::interval(Duration::from_micros(media_interval_micros(*active_fps)));
        ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
        adapted = true;
    }
    if adapted {
        println!(
            "publisher-adapt stream_id={} bitrate_bps={} fps={} frame_bytes={}",
            feedback.stream_id,
            encoder.config.bitrate_bps,
            *active_fps,
            encoder.config.synthetic_payload_bytes
        );
    }
}

fn synthetic_payload_bytes_for_bitrate(bitrate_bps: u32, frames_per_second: u16) -> usize {
    let fps = frames_per_second.max(1) as u32;
    let bytes = bitrate_bps.saturating_div(8).saturating_div(fps);
    (bytes as usize).max(MIN_SYNTHETIC_PAYLOAD_BYTES)
}

fn media_interval_micros(frames_per_second: u16) -> u64 {
    1_000_000 / frames_per_second.max(1) as u64
}

fn unix_time_micros() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_micros()
        .min(u64::MAX as u128) as u64
}

async fn poll_publisher_feedback(
    control: &mut crate::transport::quic::ControlClient,
    room_id: RoomId,
    stream_id: StreamId,
) -> anyhow::Result<PublisherFeedback> {
    let response = control
        .send(ClientControl::PollPublisherFeedback(
            PollPublisherFeedback { room_id, stream_id },
        ))
        .await?;
    print_control_response("publisher-feedback", &response);
    match response.message {
        ServerControl::PublisherFeedback(feedback) => Ok(feedback),
        ServerControl::Error(error) => bail!("poll publisher feedback failed: {}", error.message),
        other => bail!("unexpected publisher feedback response: {other:?}"),
    }
}

async fn run_synthetic_viewer_media(
    control: &mut crate::transport::quic::ControlClient,
    args: &Args,
) -> anyhow::Result<()> {
    let room_id = args
        .room_id
        .context("viewer mode requires --room-id for media receive")?;
    let target_frames = args.synthetic_media_frames()?;
    let frame_interval = args.media_frame_interval()?;
    let frame_interval_ms = frame_interval.as_millis().min(u16::MAX as u128) as u16;
    let mut buffer = FrameReassemblyBuffer::with_limits(64, args.reassembly_window_frames);
    let mut decoder = H264Decoder::default();
    let mut playback = NullPlayback;
    let mut stats = ClientMediaStats::default();
    let mut reassembled_frames = 0_u32;
    let mut decoded_frames = 0_u32;
    let mut received_packets = 0_u64;
    let mut awaiting_recovery_keyframe = false;

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
        let lost_packets_before = stats.lost_packets;
        stats.record_packet(&packet);
        received_packets += 1;
        let outcome = buffer.push_with_stats(packet)?;
        let packet_loss_detected = stats.lost_packets > lost_packets_before;
        if outcome.dropped_frames > 0 {
            stats.record_dropped_frames(outcome.dropped_frames);
        }
        if (packet_loss_detected || outcome.dropped_frames > 0) && !awaiting_recovery_keyframe {
            request_keyframe(control, room_id, args.stream_id, KeyframeReason::PacketLoss).await?;
            awaiting_recovery_keyframe = true;
        }
        stats.jitter_buffer_ms = buffer.estimated_jitter_ms(frame_interval_ms.max(1));
        if let Some(frame) = outcome.frame {
            stats.record_estimated_latency(frame.sender_capture_time_micros, unix_time_micros());
            println!(
                "media-recv frame_id={} bytes={} keyframe={} latency_ms={}",
                frame.frame_id,
                frame.bytes.len(),
                frame.is_keyframe,
                stats.estimated_latency_ms
            );
            if let Some(decoded) = decoder.decode(&frame.bytes)? {
                playback.render(decoded)?;
                stats.record_decoded_frame();
                decoded_frames += 1;
                if frame.is_keyframe {
                    awaiting_recovery_keyframe = false;
                }
            } else {
                stats.record_dropped_frame();
                if !awaiting_recovery_keyframe {
                    request_keyframe(
                        control,
                        room_id,
                        args.stream_id,
                        KeyframeReason::DecoderRecovery,
                    )
                    .await?;
                    awaiting_recovery_keyframe = true;
                }
            }
            reassembled_frames += 1;
            if args.stats_interval_frames > 0
                && reassembled_frames.is_multiple_of(args.stats_interval_frames)
            {
                send_viewer_stats(control, room_id, args.stream_id, stats).await?;
            }
        }
    }
    if received_packets > 0 {
        send_viewer_stats(control, room_id, args.stream_id, stats).await?;
    }

    println!(
        "media-summary role=viewer frames={} decoded={} packets={} lost={} dropped={} latency_ms={}",
        reassembled_frames,
        decoded_frames,
        received_packets,
        stats.lost_packets,
        stats.dropped_frames,
        stats.estimated_latency_ms
    );
    Ok(())
}

async fn request_keyframe(
    control: &mut crate::transport::quic::ControlClient,
    room_id: RoomId,
    stream_id: StreamId,
    reason: KeyframeReason,
) -> anyhow::Result<()> {
    let response = control
        .send(ClientControl::RequestKeyframe(RequestKeyframe {
            room_id,
            stream_id,
            reason,
        }))
        .await?;
    print_control_response("request-keyframe", &response);
    match response.message {
        ServerControl::RequestKeyframe(_) => Ok(()),
        ServerControl::Error(error) => bail!("request keyframe failed: {}", error.message),
        other => bail!("unexpected request keyframe response: {other:?}"),
    }
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
        Ok(Duration::from_micros(media_interval_micros(self.media_fps)))
    }

    fn synthetic_bitrate_bps(&self) -> u32 {
        let bitrate = (self.media_frame_bytes as u128)
            .saturating_mul(self.media_fps.max(1) as u128)
            .saturating_mul(8);
        bitrate.min(u32::MAX as u128) as u32
    }

    fn synthetic_stream_config(&self, room_id: RoomId) -> StreamConfig {
        StreamConfig {
            room_id,
            stream_id: self.stream_id,
            codec: CodecId::H264,
            width: H264Encoder::default().config.width,
            height: H264Encoder::default().config.height,
            frames_per_second: self.media_fps.max(1),
            timebase_hz: 90_000,
        }
    }
}

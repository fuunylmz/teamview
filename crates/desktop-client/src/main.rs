#![allow(dead_code)]

mod app;
mod audio;
mod capture;
mod decode;
mod encode;
mod playback;
mod stats;
mod transport;

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, bail};
use clap::{Parser, ValueEnum};
use teamview_protocol::{
    PROTOCOL_VERSION,
    codec::CodecId,
    control::{
        Authenticate, ClientControl, CreateRoom, Hello, JoinRoom, KeyframeReason, LeaveRoom,
        ListRooms, ListStreams, MediaKind, Ping, PollPublisherFeedback, PollStreamConfig,
        PollStreamMetrics, PublishStream, PublisherFeedback, RequestKeyframe, RoomId, RoomSummary,
        ServerControl, ServerEnvelope, SetTargetBitrate, SetTargetFramerate, StreamConfig,
        StreamId, StreamMetricsSnapshot, StreamSummary, SubscribeStream, UnsubscribeStream,
        ViewerStatsReport,
    },
    frame::{packetize_frame_for_datagram_target, packetize_frame_with_type_for_datagram_target},
    packet::{DEFAULT_DATAGRAM_PAYLOAD_TARGET, MediaPacket, PacketType},
};
use tokio::time::MissedTickBehavior;
use tracing::info;

use crate::{
    audio::{LatestAudioPlayback, SyntheticOpusDecoder, SyntheticOpusEncoder},
    capture::{
        CaptureConfig, CaptureSource, CaptureSourceInfo, CaptureSourceKind, ScreenCapture, windows,
    },
    decode::{FrameReassemblyBuffer, VideoDecoder, h264::H264Decoder},
    encode::{VideoEncoder, h264::H264Encoder},
    playback::{FramePlayback, VideoPlayback},
    stats::{ClientBroadcasterStats, ClientMediaStats},
    transport::quic::{build_client_endpoint, connect_control_client},
};

const MIN_SYNTHETIC_PAYLOAD_BYTES: usize = 64;
const CONTROL_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Mode {
    Broadcaster,
    Viewer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum CaptureSourceArg {
    PrimaryMonitor,
    Monitor,
    Window,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum MediaKindArg {
    Screen,
    Voice,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum ScreenInputArg {
    Synthetic,
    Live,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum RenderOutputArg {
    Sink,
    Window,
}

#[derive(Debug, Parser)]
#[command(author, version, about = "TeamView native desktop client scaffold")]
struct Args {
    #[arg(long, value_enum, default_value_t = Mode::Viewer)]
    mode: Mode,

    #[arg(long, default_value = "127.0.0.1:4433")]
    relay: String,

    #[arg(long)]
    access_token: Option<String>,

    #[arg(long)]
    list_capture_sources: bool,

    #[arg(long, value_enum, default_value_t = CaptureSourceArg::PrimaryMonitor)]
    capture_source: CaptureSourceArg,

    #[arg(long)]
    monitor_id: Option<String>,

    #[arg(long)]
    window_title: Option<String>,

    #[arg(long, default_value_t = true)]
    cursor_visible: bool,

    #[arg(long)]
    room_id: Option<RoomId>,

    #[arg(long, default_value = "stage1")]
    room_name: String,

    #[arg(long, default_value_t = 1)]
    stream_id: StreamId,

    #[arg(long, value_enum, default_value_t = MediaKindArg::Screen)]
    media_kind: MediaKindArg,

    #[arg(long, value_enum, default_value_t = ScreenInputArg::Synthetic)]
    screen_input: ScreenInputArg,

    #[arg(long, value_enum, default_value_t = RenderOutputArg::Sink)]
    render_output: RenderOutputArg,

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
    if args.list_capture_sources {
        print_capture_sources()?;
        return Ok(());
    }

    let endpoint = build_client_endpoint("127.0.0.1:0")?;
    let local_addr = endpoint.local_addr()?;
    let capture_supported = windows::is_supported();

    info!(
        ?args.mode,
        relay = %args.relay,
        local = %local_addr,
        capture_supported,
        ?args.capture_source,
        ?args.screen_input,
        ?args.render_output,
        cursor_visible = args.cursor_visible,
        "desktop client endpoint and capture foundation ready"
    );
    println!(
        "desktop-client mode={:?} relay={} local={} capture_supported={} capture_source={:?} screen_input={:?} render_output={:?}",
        args.mode,
        args.relay,
        local_addr,
        capture_supported,
        args.capture_source,
        args.screen_input,
        args.render_output
    );

    let mut control = connect_control_client(&endpoint, &args.relay).await?;
    let response = control
        .send(ClientControl::Hello(Hello {
            protocol_version: PROTOCOL_VERSION,
            client_name: format!("desktop-client/{:?}", args.mode),
        }))
        .await?;
    print_control_response("hello", &response);
    ensure_not_error("hello", &response)?;
    if let Some(access_token) = &args.access_token {
        authenticate_control(&mut control, access_token).await?;
    }

    match args.mode {
        Mode::Broadcaster => run_broadcaster_control_flow(&mut control, &args).await?,
        Mode::Viewer => run_viewer_control_flow(&mut control, &args).await?,
    }

    Ok(())
}

async fn authenticate_control(
    control: &mut crate::transport::quic::ControlClient,
    access_token: &str,
) -> anyhow::Result<()> {
    let response = control
        .send(ClientControl::Authenticate(Authenticate {
            token: access_token.to_owned(),
        }))
        .await?;
    print_control_response("authenticate", &response);
    match response.message {
        ServerControl::Authenticated(_) => Ok(()),
        ServerControl::Error(error) => bail!("authenticate failed: {}", error.message),
        other => bail!("unexpected authenticate response: {other:?}"),
    }
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
            codec: args.codec(),
            media_kind: args.protocol_media_kind(),
        }))
        .await?;
    print_control_response("publish-stream", &published);
    ensure_not_error("publish stream", &published)?;

    let stream_config = args.stream_config(room_id)?;
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
    leave_room(control, room_id).await?;
    Ok(())
}

async fn run_viewer_control_flow(
    control: &mut crate::transport::quic::ControlClient,
    args: &Args,
) -> anyhow::Result<()> {
    let room_id = resolve_viewer_room_id(control, args).await?;

    let joined = control
        .send(ClientControl::JoinRoom(JoinRoom { room_id }))
        .await?;
    print_control_response("join-room", &joined);
    ensure_not_error("join room", &joined)?;

    let available_streams = list_room_streams(control, room_id).await?;
    let stream_id = resolve_viewer_stream_id(args, &available_streams)?;

    let subscribed = control
        .send(ClientControl::SubscribeStream(SubscribeStream {
            room_id,
            stream_id,
        }))
        .await?;
    print_control_response("subscribe-stream", &subscribed);
    ensure_not_error("subscribe stream", &subscribed)?;

    let stream_config = poll_stream_config(control, room_id, stream_id).await?;
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
        room_id, stream_id
    );
    if args.synthetic_media_enabled() {
        run_synthetic_viewer_media(control, args, room_id, stream_id).await?;
    }
    unsubscribe_stream(control, room_id, stream_id).await?;
    leave_room(control, room_id).await?;
    Ok(())
}

async fn resolve_viewer_room_id(
    control: &mut crate::transport::quic::ControlClient,
    args: &Args,
) -> anyhow::Result<RoomId> {
    if let Some(room_id) = args.room_id {
        return Ok(room_id);
    }

    let rooms = list_rooms(control).await?;
    if rooms.is_empty() {
        bail!("no rooms are available; start a broadcaster first or pass --room-id");
    }
    let selected = select_viewer_room(&rooms, &args.room_name).with_context(|| {
        format!(
            "no room matched name {:?}; available rooms: {}",
            args.room_name,
            format_room_summaries(&rooms)
        )
    })?;
    println!(
        "room-discovery selected_room_id={} name={} participants={} streams={}",
        selected.room_id,
        selected.name,
        selected.participant_count,
        selected.published_stream_count
    );
    Ok(selected.room_id)
}

async fn list_rooms(
    control: &mut crate::transport::quic::ControlClient,
) -> anyhow::Result<Vec<RoomSummary>> {
    let response = control.send(ClientControl::ListRooms(ListRooms)).await?;
    print_control_response("list-rooms", &response);
    match response.message {
        ServerControl::RoomList(list) => Ok(list.rooms),
        ServerControl::Error(error) => bail!("list rooms failed: {}", error.message),
        other => bail!("unexpected list rooms response: {other:?}"),
    }
}

fn select_viewer_room<'a>(rooms: &'a [RoomSummary], room_name: &str) -> Option<&'a RoomSummary> {
    let mut candidates = rooms
        .iter()
        .filter(|room| room.name == room_name)
        .collect::<Vec<_>>();
    if candidates.is_empty() && rooms.len() == 1 {
        candidates.push(&rooms[0]);
    }
    candidates.into_iter().max_by_key(|room| {
        (
            room.published_stream_count > 0,
            room.participant_count > 0,
            room.room_id,
        )
    })
}

async fn list_room_streams(
    control: &mut crate::transport::quic::ControlClient,
    room_id: RoomId,
) -> anyhow::Result<Vec<StreamSummary>> {
    let response = control
        .send(ClientControl::ListStreams(ListStreams { room_id }))
        .await?;
    print_control_response("list-streams", &response);
    match response.message {
        ServerControl::StreamList(list) => Ok(list.streams),
        ServerControl::Error(error) => bail!("list streams failed: {}", error.message),
        other => bail!("unexpected list streams response: {other:?}"),
    }
}

fn resolve_viewer_stream_id(args: &Args, streams: &[StreamSummary]) -> anyhow::Result<StreamId> {
    let expected_kind = args.protocol_media_kind();
    if let Some(stream) = streams
        .iter()
        .find(|stream| stream.stream_id == args.stream_id)
    {
        if stream.media_kind != expected_kind {
            bail!(
                "stream {} is {:?}, but viewer requested {:?}",
                args.stream_id,
                stream.media_kind,
                expected_kind
            );
        }
        println!(
            "stream-discovery selected_stream_id={} codec={:?} media_kind={:?} subscribers={} configured={}",
            stream.stream_id,
            stream.codec,
            stream.media_kind,
            stream.subscriber_count,
            stream.has_config
        );
        return Ok(stream.stream_id);
    }

    let matching_streams = streams
        .iter()
        .filter(|stream| stream.media_kind == expected_kind)
        .collect::<Vec<_>>();
    match matching_streams.as_slice() {
        [] => bail!(
            "stream {} was not found and no {:?} streams are available; streams: {}",
            args.stream_id,
            expected_kind,
            format_stream_summaries(streams)
        ),
        [stream] => {
            println!(
                "stream-discovery selected_stream_id={} requested_stream_id={} codec={:?} media_kind={:?} subscribers={} configured={}",
                stream.stream_id,
                args.stream_id,
                stream.codec,
                stream.media_kind,
                stream.subscriber_count,
                stream.has_config
            );
            Ok(stream.stream_id)
        }
        _ => bail!(
            "stream {} was not found and multiple {:?} streams are available; pass --stream-id. streams: {}",
            args.stream_id,
            expected_kind,
            format_stream_summaries(streams)
        ),
    }
}

fn format_room_summaries(rooms: &[RoomSummary]) -> String {
    if rooms.is_empty() {
        return "none".to_owned();
    }
    rooms
        .iter()
        .map(|room| {
            format!(
                "{}:{} participants={} streams={}",
                room.room_id, room.name, room.participant_count, room.published_stream_count
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn format_stream_summaries(streams: &[StreamSummary]) -> String {
    if streams.is_empty() {
        return "none".to_owned();
    }
    streams
        .iter()
        .map(|stream| {
            format!(
                "{}:{:?}/{:?} subscribers={} configured={}",
                stream.stream_id,
                stream.media_kind,
                stream.codec,
                stream.subscriber_count,
                stream.has_config
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
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

async fn unsubscribe_stream(
    control: &mut crate::transport::quic::ControlClient,
    room_id: RoomId,
    stream_id: StreamId,
) -> anyhow::Result<()> {
    let response = control
        .send(ClientControl::UnsubscribeStream(UnsubscribeStream {
            room_id,
            stream_id,
        }))
        .await?;
    print_control_response("unsubscribe-stream", &response);
    match response.message {
        ServerControl::StreamUnsubscribed(_) => Ok(()),
        ServerControl::Error(error) => bail!("unsubscribe stream failed: {}", error.message),
        other => bail!("unexpected unsubscribe response: {other:?}"),
    }
}

async fn leave_room(
    control: &mut crate::transport::quic::ControlClient,
    room_id: RoomId,
) -> anyhow::Result<()> {
    let response = control
        .send(ClientControl::LeaveRoom(LeaveRoom { room_id }))
        .await?;
    print_control_response("leave-room", &response);
    match response.message {
        ServerControl::RoomLeft(_) => Ok(()),
        ServerControl::Error(error) => bail!("leave room failed: {}", error.message),
        other => bail!("unexpected leave room response: {other:?}"),
    }
}

async fn run_synthetic_broadcaster_media(
    control: &mut crate::transport::quic::ControlClient,
    args: &Args,
    room_id: RoomId,
) -> anyhow::Result<()> {
    match args.media_kind {
        MediaKindArg::Screen => {
            run_synthetic_screen_broadcaster_media(control, args, room_id).await
        }
        MediaKindArg::Voice => run_synthetic_voice_broadcaster_media(control, args, room_id).await,
    }
}

async fn run_synthetic_screen_broadcaster_media(
    control: &mut crate::transport::quic::ControlClient,
    args: &Args,
    room_id: RoomId,
) -> anyhow::Result<()> {
    if args.media_start_delay_ms > 0 {
        sleep_with_keepalive(control, Duration::from_millis(args.media_start_delay_ms)).await?;
    }
    let media_frames = args.synthetic_media_frames()?;
    let frame_interval = args.media_frame_interval()?;

    let mut capture =
        windows::WindowsGraphicsCapture::new(args.capture_source()?, args.capture_config())?;
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
    let mut timing = ClientBroadcasterStats::default();
    for frame_id in 1..=media_frames {
        ticker.tick().await;
        let capture_start = Instant::now();
        let Some(captured) = capture_screen_frame(&mut capture, args)? else {
            continue;
        };
        let capture_duration = capture_start.elapsed();
        timing.record_capture_duration(capture_duration);
        let captured_width = captured.width;
        let captured_height = captured.height;
        let encode_start = Instant::now();
        let Some(frame) = encoder.encode(captured, args.stream_id)? else {
            timing.record_encode_duration(encode_start.elapsed());
            continue;
        };
        let encode_duration = encode_start.elapsed();
        timing.record_encode_duration(encode_duration);
        let packetize_start = Instant::now();
        let packets = packetize_frame_for_datagram_target(
            &frame,
            next_sequence_number,
            args.max_datagram_payload,
        )?;
        let packetize_duration = packetize_start.elapsed();
        timing.record_packetize_duration(packetize_duration);
        next_sequence_number = next_sequence_number.wrapping_add(packets.len() as u32);
        let send_start = Instant::now();
        for packet in &packets {
            control.send_media_packet(packet)?;
            sent_packets += 1;
        }
        let send_duration = send_start.elapsed();
        timing.record_send_duration(send_duration);
        println!(
            "media-send frame_id={} fragments={} bytes={} target_bytes={} screen_input={:?} capture_width={} capture_height={} capture_ms={} encode_ms={} packetize_ms={} send_ms={}",
            frame.frame_id,
            packets.len(),
            frame.bytes.len(),
            args.media_frame_bytes,
            args.screen_input,
            captured_width,
            captured_height,
            millis_for_log(capture_duration),
            millis_for_log(encode_duration),
            millis_for_log(packetize_duration),
            millis_for_log(send_duration)
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
    let stream_metrics = poll_stream_metrics(control, room_id, args.stream_id).await?;
    println!(
        "stream-metrics stream_id={} ingress_packets={} ingress_bytes={} egress_queued={} egress_dropped={} egress_queue_packets={} egress_queue_media_ms={} subscribers={} last_ingress_time_micros={} server_route_ms_p50={} server_route_ms_p95={}",
        stream_metrics.stream_id,
        stream_metrics.ingress_packets,
        stream_metrics.ingress_bytes,
        stream_metrics.egress_queued_packets,
        stream_metrics.egress_dropped_packets,
        stream_metrics.egress_queue_packets,
        stream_metrics.egress_queue_media_ms,
        stream_metrics.subscriber_count,
        stream_metrics.last_ingress_time_micros,
        stream_metrics.server_route_ms_p50,
        stream_metrics.server_route_ms_p95
    );
    let timing = timing.timing_snapshot();
    println!(
        "media-summary role=broadcaster frames={} packets={} fps={} run_ms={} capture_ms_p50={} capture_ms_p95={} encode_ms_p50={} encode_ms_p95={} packetize_ms_p50={} packetize_ms_p95={} send_ms_p50={} send_ms_p95={}",
        media_frames,
        sent_packets,
        args.media_fps,
        args.media_run_ms,
        timing.capture_ms_p50,
        timing.capture_ms_p95,
        timing.encode_ms_p50,
        timing.encode_ms_p95,
        timing.packetize_ms_p50,
        timing.packetize_ms_p95,
        timing.send_ms_p50,
        timing.send_ms_p95
    );
    if args.media_end_linger_ms > 0 {
        tokio::time::sleep(Duration::from_millis(args.media_end_linger_ms)).await;
    }
    Ok(())
}

fn capture_screen_frame(
    capture: &mut windows::WindowsGraphicsCapture,
    args: &Args,
) -> anyhow::Result<Option<capture::CaptureFrame>> {
    match args.screen_input {
        ScreenInputArg::Synthetic => {
            capture.push_test_frame(1280, 720, unix_time_micros());
            capture.next_frame()
        }
        ScreenInputArg::Live => capture.next_frame(),
    }
}

async fn run_synthetic_voice_broadcaster_media(
    control: &mut crate::transport::quic::ControlClient,
    args: &Args,
    room_id: RoomId,
) -> anyhow::Result<()> {
    if args.media_start_delay_ms > 0 {
        sleep_with_keepalive(control, Duration::from_millis(args.media_start_delay_ms)).await?;
    }
    let media_frames = args.synthetic_media_frames()?;
    let frame_interval = args.media_frame_interval()?;
    let mut encoder = SyntheticOpusEncoder::default();
    encoder.config.synthetic_payload_bytes = args.media_frame_bytes;
    encoder.config.bitrate_bps = args.synthetic_bitrate_bps();
    encoder.set_frames_per_second(args.media_fps.max(1));

    let mut ticker = tokio::time::interval(frame_interval);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut active_fps = args.media_fps;
    let mut sent_packets = 0_u64;
    let mut next_sequence_number = 1_u32;
    let mut timing = ClientBroadcasterStats::default();
    for frame_id in 1..=media_frames {
        ticker.tick().await;
        let capture_start = Instant::now();
        let capture_time_micros = unix_time_micros();
        let capture_duration = capture_start.elapsed();
        timing.record_capture_duration(capture_duration);
        let encode_start = Instant::now();
        let frame = encoder.encode(frame_id, capture_time_micros, args.stream_id)?;
        let encode_duration = encode_start.elapsed();
        timing.record_encode_duration(encode_duration);
        let packetize_start = Instant::now();
        let packets = packetize_frame_with_type_for_datagram_target(
            &frame,
            PacketType::Audio,
            next_sequence_number,
            args.max_datagram_payload,
        )?;
        let packetize_duration = packetize_start.elapsed();
        timing.record_packetize_duration(packetize_duration);
        next_sequence_number = next_sequence_number.wrapping_add(packets.len() as u32);
        let send_start = Instant::now();
        for packet in &packets {
            control.send_media_packet(packet)?;
            sent_packets += 1;
        }
        let send_duration = send_start.elapsed();
        timing.record_send_duration(send_duration);
        println!(
            "audio-send frame_id={} fragments={} bytes={} target_bytes={} capture_ms={} encode_ms={} packetize_ms={} send_ms={}",
            frame.frame_id,
            packets.len(),
            frame.bytes.len(),
            encoder.config.synthetic_payload_bytes,
            millis_for_log(capture_duration),
            millis_for_log(encode_duration),
            millis_for_log(packetize_duration),
            millis_for_log(send_duration)
        );
        if args.feedback_interval_frames > 0
            && frame_id.is_multiple_of(args.feedback_interval_frames)
        {
            let feedback = poll_publisher_feedback(control, room_id, args.stream_id).await?;
            apply_audio_publisher_feedback(&feedback, &mut encoder, &mut active_fps, &mut ticker);
        }
    }
    let feedback = poll_publisher_feedback(control, room_id, args.stream_id).await?;
    apply_audio_publisher_feedback(&feedback, &mut encoder, &mut active_fps, &mut ticker);
    let stream_metrics = poll_stream_metrics(control, room_id, args.stream_id).await?;
    println!(
        "stream-metrics stream_id={} ingress_packets={} ingress_bytes={} egress_queued={} egress_dropped={} egress_queue_packets={} egress_queue_media_ms={} subscribers={} last_ingress_time_micros={} server_route_ms_p50={} server_route_ms_p95={}",
        stream_metrics.stream_id,
        stream_metrics.ingress_packets,
        stream_metrics.ingress_bytes,
        stream_metrics.egress_queued_packets,
        stream_metrics.egress_dropped_packets,
        stream_metrics.egress_queue_packets,
        stream_metrics.egress_queue_media_ms,
        stream_metrics.subscriber_count,
        stream_metrics.last_ingress_time_micros,
        stream_metrics.server_route_ms_p50,
        stream_metrics.server_route_ms_p95
    );
    let timing = timing.timing_snapshot();
    println!(
        "media-summary role=broadcaster kind=voice frames={} packets={} fps={} run_ms={} capture_ms_p50={} capture_ms_p95={} encode_ms_p50={} encode_ms_p95={} packetize_ms_p50={} packetize_ms_p95={} send_ms_p50={} send_ms_p95={}",
        media_frames,
        sent_packets,
        args.media_fps,
        args.media_run_ms,
        timing.capture_ms_p50,
        timing.capture_ms_p95,
        timing.encode_ms_p50,
        timing.encode_ms_p95,
        timing.packetize_ms_p50,
        timing.packetize_ms_p95,
        timing.send_ms_p50,
        timing.send_ms_p95
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

fn apply_audio_publisher_feedback(
    feedback: &PublisherFeedback,
    encoder: &mut SyntheticOpusEncoder,
    active_fps: &mut u16,
    ticker: &mut tokio::time::Interval,
) {
    let mut adapted = false;
    let target_bitrate = feedback.aggregate_available_bitrate_bps;
    if target_bitrate > 0 && target_bitrate != encoder.config.bitrate_bps {
        encoder.update_bitrate(target_bitrate);
        adapted = true;
    }
    if feedback.target_frames_per_second > 0 && feedback.target_frames_per_second != *active_fps {
        *active_fps = feedback.target_frames_per_second;
        encoder.set_frames_per_second(*active_fps);
        *ticker = tokio::time::interval(Duration::from_micros(media_interval_micros(*active_fps)));
        ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
        adapted = true;
    }
    if adapted {
        println!(
            "publisher-adapt stream_id={} bitrate_bps={} fps={} audio_bytes={}",
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

fn millis_for_log(duration: Duration) -> u16 {
    duration.as_millis().min(u16::MAX as u128) as u16
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

async fn poll_stream_metrics(
    control: &mut crate::transport::quic::ControlClient,
    room_id: RoomId,
    stream_id: StreamId,
) -> anyhow::Result<StreamMetricsSnapshot> {
    let response = control
        .send(ClientControl::PollStreamMetrics(PollStreamMetrics {
            room_id,
            stream_id,
        }))
        .await?;
    print_control_response("stream-metrics", &response);
    match response.message {
        ServerControl::StreamMetrics(metrics) => Ok(metrics),
        ServerControl::Error(error) => bail!("poll stream metrics failed: {}", error.message),
        other => bail!("unexpected stream metrics response: {other:?}"),
    }
}

async fn run_synthetic_viewer_media(
    control: &mut crate::transport::quic::ControlClient,
    args: &Args,
    room_id: RoomId,
    stream_id: StreamId,
) -> anyhow::Result<()> {
    match args.media_kind {
        MediaKindArg::Screen => {
            run_synthetic_screen_viewer_media(control, args, room_id, stream_id).await
        }
        MediaKindArg::Voice => {
            run_synthetic_voice_viewer_media(control, args, room_id, stream_id).await
        }
    }
}

async fn run_synthetic_screen_viewer_media(
    control: &mut crate::transport::quic::ControlClient,
    args: &Args,
    room_id: RoomId,
    stream_id: StreamId,
) -> anyhow::Result<()> {
    let target_frames = args.synthetic_media_frames()?;
    let frame_interval = args.media_frame_interval()?;
    let frame_interval_ms = frame_interval.as_millis().min(u16::MAX as u128) as u16;
    let mut buffer = FrameReassemblyBuffer::with_limits(64, args.reassembly_window_frames);
    let mut decoder = H264Decoder::default();
    let mut playback = args.video_playback()?;
    let mut stats = ClientMediaStats::default();
    let mut reassembled_frames = 0_u32;
    let mut decoded_frames = 0_u32;
    let mut received_packets = 0_u64;
    let mut awaiting_recovery_keyframe = false;

    while reassembled_frames < target_frames {
        let packet = match recv_media_packet_with_keepalive(
            control,
            Duration::from_millis(args.media_idle_timeout_ms),
        )
        .await?
        {
            Some(packet) => packet,
            None => {
                if args.media_frames == 0 && received_packets > 0 {
                    break;
                }
                bail!("timed out waiting for media packet");
            }
        };
        let packet_receive_time_micros = unix_time_micros();
        let lost_packets_before = stats.lost_packets;
        stats.record_packet(&packet);
        received_packets += 1;
        let outcome = buffer.push_with_stats_at(packet, packet_receive_time_micros)?;
        let packet_loss_detected = stats.lost_packets > lost_packets_before;
        if outcome.dropped_frames > 0 {
            stats.record_dropped_frames(outcome.dropped_frames);
        }
        if (packet_loss_detected || outcome.dropped_frames > 0) && !awaiting_recovery_keyframe {
            request_keyframe(control, room_id, stream_id, KeyframeReason::PacketLoss).await?;
            awaiting_recovery_keyframe = true;
        }
        stats.jitter_buffer_ms = buffer.estimated_jitter_ms(frame_interval_ms.max(1));
        if let Some(frame) = outcome.frame {
            stats.record_reassembly_millis(outcome.reassembly_ms);
            stats.record_estimated_latency(
                frame.sender_capture_time_micros,
                packet_receive_time_micros,
            );
            stats.record_server_queue_latency(
                frame.server_receive_time_micros,
                frame.server_send_time_micros,
            );
            println!(
                "media-recv frame_id={} bytes={} keyframe={} latency_ms={} server_queue_ms={} reassembly_ms={}",
                frame.frame_id,
                frame.bytes.len(),
                frame.is_keyframe,
                stats.estimated_latency_ms,
                stats.server_queue_ms,
                outcome.reassembly_ms
            );
            let decode_start = Instant::now();
            if let Some(decoded) = decoder.decode(&frame.bytes)? {
                let decode_duration = decode_start.elapsed();
                stats.record_decode_duration(decode_duration);
                let render_start = Instant::now();
                playback.render(decoded)?;
                let render_duration = render_start.elapsed();
                if let Some(rendered) = playback.latest_frame() {
                    stats.record_render_duration(render_duration, rendered.render_time_micros);
                    println!(
                        "media-render frame_id={} width={} height={} pixel_bytes={} render_time_micros={} decode_ms={} render_ms={} render_fps={}",
                        rendered.frame_id,
                        rendered.width,
                        rendered.height,
                        rendered.pixel_bytes,
                        rendered.render_time_micros,
                        millis_for_log(decode_duration),
                        millis_for_log(render_duration),
                        stats.render_fps()
                    );
                }
                stats.record_decoded_frame();
                decoded_frames += 1;
                if frame.is_keyframe {
                    awaiting_recovery_keyframe = false;
                }
            } else {
                stats.record_dropped_frame();
                if !awaiting_recovery_keyframe {
                    request_keyframe(control, room_id, stream_id, KeyframeReason::DecoderRecovery)
                        .await?;
                    awaiting_recovery_keyframe = true;
                }
            }
            reassembled_frames += 1;
            if args.stats_interval_frames > 0
                && reassembled_frames.is_multiple_of(args.stats_interval_frames)
            {
                send_viewer_stats(control, room_id, stream_id, stats).await?;
            }
        }
    }
    if received_packets > 0 {
        send_viewer_stats(control, room_id, stream_id, stats).await?;
    }

    println!(
        "media-summary role=viewer frames={} decoded={} rendered={} packets={} lost={} dropped={} latency_ms={} server_queue_ms_p50={} server_queue_ms_p95={} reassembly_ms_p50={} reassembly_ms_p95={} decode_ms_p50={} decode_ms_p95={} render_ms_p50={} render_ms_p95={} render_fps={}",
        reassembled_frames,
        decoded_frames,
        playback.rendered_frames(),
        received_packets,
        stats.lost_packets,
        stats.dropped_frames,
        stats.estimated_latency_ms,
        stats.server_queue_ms_p50(),
        stats.server_queue_ms_p95(),
        stats.to_viewer_report(room_id, stream_id).reassembly_ms_p50,
        stats.to_viewer_report(room_id, stream_id).reassembly_ms_p95,
        stats.to_viewer_report(room_id, stream_id).decode_ms_p50,
        stats.to_viewer_report(room_id, stream_id).decode_ms_p95,
        stats.to_viewer_report(room_id, stream_id).render_ms_p50,
        stats.to_viewer_report(room_id, stream_id).render_ms_p95,
        stats.render_fps()
    );
    Ok(())
}

async fn run_synthetic_voice_viewer_media(
    control: &mut crate::transport::quic::ControlClient,
    args: &Args,
    room_id: RoomId,
    stream_id: StreamId,
) -> anyhow::Result<()> {
    let target_frames = args.synthetic_media_frames()?;
    let frame_interval = args.media_frame_interval()?;
    let frame_interval_ms = frame_interval.as_millis().min(u16::MAX as u128) as u16;
    let mut buffer = FrameReassemblyBuffer::with_limits(64, args.reassembly_window_frames);
    let mut decoder = SyntheticOpusDecoder;
    let mut playback = LatestAudioPlayback::default();
    let mut stats = ClientMediaStats::default();
    let mut reassembled_frames = 0_u32;
    let mut decoded_frames = 0_u32;
    let mut received_packets = 0_u64;

    while reassembled_frames < target_frames {
        let packet = match recv_media_packet_with_keepalive(
            control,
            Duration::from_millis(args.media_idle_timeout_ms),
        )
        .await?
        {
            Some(packet) => packet,
            None => {
                if args.media_frames == 0 && received_packets > 0 {
                    break;
                }
                bail!("timed out waiting for audio packet");
            }
        };
        let packet_receive_time_micros = unix_time_micros();
        stats.record_packet(&packet);
        received_packets += 1;
        let outcome = buffer.push_with_stats_at(packet, packet_receive_time_micros)?;
        if outcome.dropped_frames > 0 {
            stats.record_dropped_frames(outcome.dropped_frames);
        }
        stats.jitter_buffer_ms = buffer.estimated_jitter_ms(frame_interval_ms.max(1));
        if let Some(frame) = outcome.frame {
            stats.record_reassembly_millis(outcome.reassembly_ms);
            stats.record_estimated_latency(
                frame.sender_capture_time_micros,
                packet_receive_time_micros,
            );
            stats.record_server_queue_latency(
                frame.server_receive_time_micros,
                frame.server_send_time_micros,
            );
            println!(
                "audio-recv frame_id={} bytes={} latency_ms={} server_queue_ms={} reassembly_ms={}",
                frame.frame_id,
                frame.bytes.len(),
                stats.estimated_latency_ms,
                stats.server_queue_ms,
                outcome.reassembly_ms
            );
            let decode_start = Instant::now();
            if let Some(decoded) = decoder.decode(&frame.bytes)? {
                let decode_duration = decode_start.elapsed();
                stats.record_decode_duration(decode_duration);
                let play_start = Instant::now();
                playback.play(decoded)?;
                let play_duration = play_start.elapsed();
                stats.record_render_duration(play_duration, unix_time_micros());
                if let Some(played) = playback.latest() {
                    println!(
                        "audio-play frame_id={} sample_rate_hz={} channels={} samples={} decode_ms={} play_ms={} play_fps={}",
                        played.frame_id,
                        played.sample_rate_hz,
                        played.channel_count,
                        played.sample_count,
                        millis_for_log(decode_duration),
                        millis_for_log(play_duration),
                        stats.render_fps()
                    );
                }
                stats.record_decoded_frame();
                decoded_frames += 1;
            } else {
                stats.record_dropped_frame();
            }
            reassembled_frames += 1;
            if args.stats_interval_frames > 0
                && reassembled_frames.is_multiple_of(args.stats_interval_frames)
            {
                send_viewer_stats(control, room_id, stream_id, stats).await?;
            }
        }
    }
    if received_packets > 0 {
        send_viewer_stats(control, room_id, stream_id, stats).await?;
    }

    println!(
        "media-summary role=viewer kind=voice frames={} decoded={} played={} packets={} lost={} dropped={} latency_ms={} server_queue_ms_p50={} server_queue_ms_p95={} reassembly_ms_p50={} reassembly_ms_p95={} decode_ms_p50={} decode_ms_p95={} play_ms_p50={} play_ms_p95={} play_fps={}",
        reassembled_frames,
        decoded_frames,
        playback.played_frames(),
        received_packets,
        stats.lost_packets,
        stats.dropped_frames,
        stats.estimated_latency_ms,
        stats.server_queue_ms_p50(),
        stats.server_queue_ms_p95(),
        stats.to_viewer_report(room_id, stream_id).reassembly_ms_p50,
        stats.to_viewer_report(room_id, stream_id).reassembly_ms_p95,
        stats.to_viewer_report(room_id, stream_id).decode_ms_p50,
        stats.to_viewer_report(room_id, stream_id).decode_ms_p95,
        stats.to_viewer_report(room_id, stream_id).render_ms_p50,
        stats.to_viewer_report(room_id, stream_id).render_ms_p95,
        stats.render_fps()
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

async fn sleep_with_keepalive(
    control: &mut crate::transport::quic::ControlClient,
    duration: Duration,
) -> anyhow::Result<()> {
    let mut remaining = duration;
    while remaining > Duration::ZERO {
        let chunk = remaining.min(CONTROL_KEEPALIVE_INTERVAL);
        tokio::time::sleep(chunk).await;
        remaining = remaining.saturating_sub(chunk);
        if remaining > Duration::ZERO {
            send_keepalive(control).await?;
        }
    }
    Ok(())
}

async fn recv_media_packet_with_keepalive(
    control: &mut crate::transport::quic::ControlClient,
    idle_timeout: Duration,
) -> anyhow::Result<Option<MediaPacket>> {
    let mut remaining = idle_timeout;
    while remaining > Duration::ZERO {
        let chunk = remaining.min(CONTROL_KEEPALIVE_INTERVAL);
        match tokio::time::timeout(chunk, control.recv_media_packet()).await {
            Ok(packet) => return packet.map(Some),
            Err(_) => {
                remaining = remaining.saturating_sub(chunk);
                if remaining > Duration::ZERO {
                    send_keepalive(control).await?;
                }
            }
        }
    }
    Ok(None)
}

async fn send_keepalive(control: &mut crate::transport::quic::ControlClient) -> anyhow::Result<()> {
    let nonce = unix_time_micros();
    let response = control.send(ClientControl::Ping(Ping { nonce })).await?;
    match response.message {
        ServerControl::Pong(pong) if pong.nonce == nonce => Ok(()),
        ServerControl::Error(error) => bail!("keepalive failed: {}", error.message),
        other => bail!("unexpected keepalive response: {other:?}"),
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

fn print_capture_sources() -> anyhow::Result<()> {
    let sources = windows::list_capture_sources()?;
    println!("capture-sources count={}", sources.len());
    for source in sources {
        println!("{}", format_capture_source(&source));
    }
    Ok(())
}

fn format_capture_source(source: &CaptureSourceInfo) -> String {
    match (&source.kind, &source.source) {
        (CaptureSourceKind::Monitor, CaptureSource::Monitor { id }) => format!(
            "capture-source kind=monitor id={} primary={} width={} height={} label={:?}",
            id, source.is_primary, source.width, source.height, source.label
        ),
        (CaptureSourceKind::PrimaryMonitor, CaptureSource::PrimaryMonitor) => format!(
            "capture-source kind=primary-monitor width={} height={} label={:?}",
            source.width, source.height, source.label
        ),
        (CaptureSourceKind::Window, CaptureSource::Window { title, .. }) => format!(
            "capture-source kind=window title={:?} width={} height={} label={:?}",
            title, source.width, source.height, source.label
        ),
        (_, capture_source) => format!(
            "capture-source kind={:?} source={:?} primary={} width={} height={} label={:?}",
            source.kind,
            capture_source,
            source.is_primary,
            source.width,
            source.height,
            source.label
        ),
    }
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

    fn protocol_media_kind(&self) -> MediaKind {
        match self.media_kind {
            MediaKindArg::Screen => MediaKind::Screen,
            MediaKindArg::Voice => MediaKind::Voice,
        }
    }

    fn codec(&self) -> CodecId {
        match self.media_kind {
            MediaKindArg::Screen => CodecId::H264,
            MediaKindArg::Voice => CodecId::Opus,
        }
    }

    fn stream_config(&self, room_id: RoomId) -> anyhow::Result<StreamConfig> {
        match self.media_kind {
            MediaKindArg::Screen => {
                let (width, height) = self.screen_capture_dimensions()?;
                Ok(StreamConfig {
                    room_id,
                    stream_id: self.stream_id,
                    codec: CodecId::H264,
                    width,
                    height,
                    frames_per_second: self.media_fps.max(1),
                    timebase_hz: 90_000,
                })
            }
            MediaKindArg::Voice => Ok(StreamConfig {
                room_id,
                stream_id: self.stream_id,
                codec: CodecId::Opus,
                width: 0,
                height: 0,
                frames_per_second: self.media_fps.max(1),
                timebase_hz: 48_000,
            }),
        }
    }

    fn screen_capture_dimensions(&self) -> anyhow::Result<(u32, u32)> {
        match self.screen_input {
            ScreenInputArg::Synthetic => {
                let config = H264Encoder::default().config;
                Ok((config.width, config.height))
            }
            ScreenInputArg::Live => windows::capture_source_size(&self.capture_source()?)
                .context("failed to query live capture source size"),
        }
    }

    fn capture_source(&self) -> anyhow::Result<CaptureSource> {
        match self.capture_source {
            CaptureSourceArg::PrimaryMonitor => Ok(CaptureSource::PrimaryMonitor),
            CaptureSourceArg::Monitor => {
                let id = self
                    .monitor_id
                    .as_deref()
                    .map(str::trim)
                    .filter(|id| !id.is_empty())
                    .ok_or_else(|| {
                        anyhow::anyhow!("--monitor-id is required when --capture-source monitor")
                    })?;
                Ok(CaptureSource::Monitor { id: id.to_owned() })
            }
            CaptureSourceArg::Window => {
                let title = self
                    .window_title
                    .as_deref()
                    .map(str::trim)
                    .filter(|title| !title.is_empty())
                    .ok_or_else(|| {
                        anyhow::anyhow!("--window-title is required when --capture-source window")
                    })?;
                Ok(CaptureSource::Window {
                    id: title.to_owned(),
                    title: title.to_owned(),
                })
            }
        }
    }

    fn capture_config(&self) -> CaptureConfig {
        CaptureConfig {
            queue_capacity: 1,
            cursor_visible: self.cursor_visible,
        }
    }

    fn video_playback(&self) -> anyhow::Result<FramePlayback> {
        match self.render_output {
            RenderOutputArg::Sink => Ok(FramePlayback::latest()),
            RenderOutputArg::Window => {
                FramePlayback::window("TeamView Viewer").context("failed to create render window")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_capture_sources_flag_parses_without_relay_options() {
        let args = Args::try_parse_from(["desktop-client", "--list-capture-sources"]).unwrap();

        assert!(args.list_capture_sources);
    }

    #[test]
    fn format_capture_source_prints_monitor_selection_hint() {
        let source = CaptureSourceInfo {
            kind: CaptureSourceKind::Monitor,
            source: CaptureSource::Monitor { id: "0".to_owned() },
            label: "Monitor 0 (primary)".to_owned(),
            width: 1920,
            height: 1080,
            is_primary: true,
        };

        assert_eq!(
            format_capture_source(&source),
            "capture-source kind=monitor id=0 primary=true width=1920 height=1080 label=\"Monitor 0 (primary)\""
        );
    }

    #[test]
    fn format_capture_source_prints_window_title() {
        let source = CaptureSourceInfo {
            kind: CaptureSourceKind::Window,
            source: CaptureSource::Window {
                id: "Untitled - Notepad".to_owned(),
                title: "Untitled - Notepad".to_owned(),
            },
            label: "Untitled - Notepad".to_owned(),
            width: 800,
            height: 600,
            is_primary: false,
        };

        assert_eq!(
            format_capture_source(&source),
            "capture-source kind=window title=\"Untitled - Notepad\" width=800 height=600 label=\"Untitled - Notepad\""
        );
    }

    #[test]
    fn monitor_capture_source_requires_id() {
        let args = Args::try_parse_from([
            "desktop-client",
            "--capture-source",
            "monitor",
            "--screen-input",
            "live",
        ])
        .unwrap();

        let error = args.capture_source().unwrap_err();

        assert!(error.to_string().contains("--monitor-id"));
    }

    #[test]
    fn monitor_capture_source_uses_monitor_id() {
        let args = Args::try_parse_from([
            "desktop-client",
            "--capture-source",
            "monitor",
            "--monitor-id",
            "0",
        ])
        .unwrap();

        assert_eq!(
            args.capture_source().unwrap(),
            CaptureSource::Monitor { id: "0".to_owned() }
        );
    }

    #[test]
    fn window_capture_source_requires_title() {
        let args = Args::try_parse_from([
            "desktop-client",
            "--capture-source",
            "window",
            "--screen-input",
            "live",
        ])
        .unwrap();

        let error = args.capture_source().unwrap_err();

        assert!(error.to_string().contains("--window-title"));
    }

    #[test]
    fn window_capture_source_uses_title_as_id_and_label() {
        let args = Args::try_parse_from([
            "desktop-client",
            "--capture-source",
            "window",
            "--window-title",
            "Calculator",
        ])
        .unwrap();

        assert_eq!(
            args.capture_source().unwrap(),
            CaptureSource::Window {
                id: "Calculator".to_owned(),
                title: "Calculator".to_owned()
            }
        );
    }
}

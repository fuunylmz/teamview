#![allow(dead_code)]

mod app;
mod audio;
mod audio_capture;
mod capture;
mod decode;
mod encode;
mod media_foundation;
mod playback;
mod remote_input;
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
        ListParticipants, ListRooms, ListStreams, MediaKind, ParticipantSummary, Ping,
        PointerButton, PollPublisherFeedback, PollRemoteInput, PollStreamConfig, PollStreamMetrics,
        PublishStream, PublisherFeedback, RemoteInputEvent, RemoteInputKind, RequestKeyframe,
        RoomId, RoomSummary, SendRemoteInput, ServerControl, ServerEnvelope, SetTargetBitrate,
        SetTargetFramerate, SetVoiceState, StreamConfig, StreamId, StreamMetricsSnapshot,
        StreamSummary, SubscribeStream, TimeSyncRequest, TimeSyncResponse, UnsubscribeStream,
        ViewerStatsReport,
    },
    frame::{packetize_frame_for_datagram_target, packetize_frame_with_type_for_datagram_target},
    packet::{DEFAULT_DATAGRAM_PAYLOAD_TARGET, MediaPacket, PacketType},
};
use tokio::time::MissedTickBehavior;
use tracing::info;

use crate::{
    audio::{AudioOutputPlayback, AudioPlayback, SyntheticOpusDecoder, SyntheticOpusEncoder},
    audio_capture::{
        AudioCaptureConfig, MicrophoneCapture, MicrophoneSource, MicrophoneSourceInfo,
        WindowsMicrophoneCapture,
    },
    capture::{
        CaptureConfig, CaptureSource, CaptureSourceInfo, CaptureSourceKind, ScreenCapture, windows,
    },
    decode::{
        FrameReassemblyBuffer, VideoDecoder,
        h264::{H264VideoDecoder, H264VideoDecoderBackend, h264_decoder_backend_status},
    },
    encode::{
        VideoEncoder,
        h264::{
            H264Encoder, H264EncoderConfig, H264VideoEncoder, H264VideoEncoderBackend,
            h264_encoder_backend_status,
        },
    },
    playback::{FramePlayback, VideoPlayback},
    remote_input::{LoggingRemoteInputApplier, NativeRemoteInputApplier, RemoteInputApplier},
    stats::{ClientBroadcasterStats, ClientMediaStats},
    transport::quic::{build_client_endpoint, connect_control_client},
};

const MIN_SYNTHETIC_PAYLOAD_BYTES: usize = 64;
const CONTROL_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(10);
const DEFAULT_TIME_SYNC_SAMPLES: u8 = 5;
const DEFAULT_TIME_SYNC_SPACING_MS: u64 = 20;
const DEFAULT_CHANNEL_NAME: &str = "stage1";
const DEFAULT_SCREEN_FPS: u16 = 30;
const DEFAULT_VOICE_FPS: u16 = 50;
const DEFAULT_VOICE_FRAME_BYTES: usize = 96;

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
    Both,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum ScreenInputArg {
    Synthetic,
    Live,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum VideoEncoderArg {
    Synthetic,
    MediaFoundation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum VideoDecoderArg {
    Synthetic,
    MediaFoundation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum VoiceInputArg {
    Synthetic,
    Microphone,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum RenderOutputArg {
    Sink,
    Window,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum AudioOutputArg {
    Sink,
    Speaker,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum RemoteInputScriptArg {
    PointerTap,
    KeyEnter,
    Text,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum RemoteInputOutputArg {
    Log,
    Native,
}

#[derive(Debug, Clone, Parser)]
#[command(author, version, about = "TeamView native desktop client scaffold")]
struct Args {
    #[arg(long, value_enum, default_value_t = Mode::Viewer)]
    mode: Mode,

    #[arg(long, default_value = "127.0.0.1:4433")]
    relay: String,

    #[arg(long)]
    access_token: Option<String>,

    #[arg(long)]
    display_name: Option<String>,

    #[arg(long)]
    list_capture_sources: bool,

    #[arg(long)]
    list_audio_sources: bool,

    #[arg(long)]
    list_codec_backends: bool,

    #[arg(long)]
    list_rooms: bool,

    #[arg(long)]
    list_streams: bool,

    #[arg(long)]
    list_participants: bool,

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

    #[arg(long)]
    channel_id: Option<RoomId>,

    #[arg(long)]
    channel_name: Option<String>,

    #[arg(long, default_value_t = 1)]
    stream_id: StreamId,

    #[arg(long)]
    voice_stream_id: Option<StreamId>,

    #[arg(long, value_enum, default_value_t = MediaKindArg::Screen)]
    media_kind: MediaKindArg,

    #[arg(long, value_enum, default_value_t = ScreenInputArg::Synthetic)]
    screen_input: ScreenInputArg,

    #[arg(long, value_enum, default_value_t = VideoEncoderArg::Synthetic)]
    video_encoder: VideoEncoderArg,

    #[arg(long, value_enum, default_value_t = VideoDecoderArg::Synthetic)]
    video_decoder: VideoDecoderArg,

    #[arg(long, value_enum, default_value_t = VoiceInputArg::Synthetic)]
    voice_input: VoiceInputArg,

    #[arg(long)]
    microphone_id: Option<String>,

    #[arg(long, value_enum, default_value_t = RenderOutputArg::Sink)]
    render_output: RenderOutputArg,

    #[arg(long, value_enum, default_value_t = AudioOutputArg::Sink)]
    audio_output: AudioOutputArg,

    #[arg(long)]
    muted: bool,

    #[arg(long)]
    deafened: bool,

    #[arg(long)]
    push_to_talk: bool,

    #[arg(long)]
    ptt_active: bool,

    #[arg(long, default_value_t = 0)]
    media_frames: u32,

    #[arg(long, default_value_t = 0)]
    media_run_ms: u64,

    #[arg(long, default_value_t = DEFAULT_SCREEN_FPS)]
    media_fps: u16,

    #[arg(long, default_value_t = DEFAULT_VOICE_FPS)]
    voice_fps: u16,

    #[arg(long, default_value_t = 512)]
    media_frame_bytes: usize,

    #[arg(long, default_value_t = DEFAULT_VOICE_FRAME_BYTES)]
    voice_frame_bytes: usize,

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

    #[arg(long, default_value_t = 150)]
    jitter_buffer_max_ms: u16,

    #[arg(long, default_value_t = 30)]
    stats_interval_frames: u32,

    #[arg(long, default_value_t = 30)]
    feedback_interval_frames: u32,

    #[arg(long, default_value_t = DEFAULT_TIME_SYNC_SAMPLES)]
    time_sync_samples: u8,

    #[arg(long, default_value_t = DEFAULT_TIME_SYNC_SPACING_MS)]
    time_sync_spacing_ms: u64,

    #[arg(long, default_value_t = 5_000)]
    time_sync_refresh_ms: u64,

    #[arg(long, value_enum)]
    remote_input_script: Option<RemoteInputScriptArg>,

    #[arg(long)]
    remote_input_text: Option<String>,

    #[arg(long, value_enum, default_value_t = RemoteInputOutputArg::Log)]
    remote_input_output: RemoteInputOutputArg,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let args = Args::parse();
    if args.list_capture_sources {
        print_capture_sources()?;
        return Ok(());
    }
    if args.list_audio_sources {
        print_audio_sources()?;
        return Ok(());
    }
    if args.list_codec_backends {
        print_codec_backends();
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
        ?args.video_encoder,
        ?args.video_decoder,
        ?args.voice_input,
        ?args.render_output,
        ?args.audio_output,
        ?args.remote_input_output,
        cursor_visible = args.cursor_visible,
        "desktop client endpoint and capture foundation ready"
    );
    println!(
        "desktop-client mode={:?} relay={} local={} capture_supported={} capture_source={:?} screen_input={:?} video_encoder={:?} video_decoder={:?} voice_input={:?} render_output={:?} audio_output={:?} remote_input_output={:?} muted={} deafened={} push_to_talk={} speaking={}",
        args.mode,
        args.relay,
        local_addr,
        capture_supported,
        args.capture_source,
        args.screen_input,
        args.video_encoder,
        args.video_decoder,
        args.voice_input,
        args.render_output,
        args.audio_output,
        args.remote_input_output,
        args.muted,
        args.deafened,
        args.voice_push_to_talk_enabled(),
        args.voice_speaking()
    );

    let control = connect_control_client(&endpoint, &args.relay).await?;
    let response = control
        .send(ClientControl::Hello(Hello {
            protocol_version: PROTOCOL_VERSION,
            client_name: args.control_display_name(),
        }))
        .await?;
    print_control_response("hello", &response);
    ensure_not_error("hello", &response)?;
    let clock_sync = sync_control_clock(&control, &args).await?;
    if let Some(access_token) = &args.access_token {
        authenticate_control(&control, access_token).await?;
    }
    if args.list_rooms {
        run_list_rooms_flow(&control).await?;
        return Ok(());
    }
    if args.list_streams {
        run_list_streams_flow(&control, &args).await?;
        return Ok(());
    }
    if args.list_participants {
        run_list_participants_flow(&control, &args).await?;
        return Ok(());
    }

    match args.mode {
        Mode::Broadcaster => run_broadcaster_control_flow(&control, &args, clock_sync).await?,
        Mode::Viewer => run_viewer_control_flow(&control, &args, clock_sync).await?,
    }

    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ClockSyncEstimate {
    rtt_micros: u64,
    rtt_ms: u16,
    clock_offset_micros: i64,
}

#[derive(Debug, Clone)]
struct ClockSyncTracker {
    estimate: ClockSyncEstimate,
    refresh_interval: Option<Duration>,
    next_refresh_at: Instant,
}

impl ClockSyncTracker {
    fn new(estimate: ClockSyncEstimate, refresh_interval: Option<Duration>) -> Self {
        Self {
            estimate,
            refresh_interval,
            next_refresh_at: refresh_interval
                .map(next_clock_sync_refresh_at)
                .unwrap_or_else(Instant::now),
        }
    }

    fn estimate(&self) -> ClockSyncEstimate {
        self.estimate
    }

    async fn refresh_if_due(
        &mut self,
        control: &crate::transport::quic::ControlClient,
        stage: &str,
    ) -> anyhow::Result<ClockSyncEstimate> {
        let Some(refresh_interval) = self.refresh_interval else {
            return Ok(self.estimate);
        };
        if Instant::now() < self.next_refresh_at {
            return Ok(self.estimate);
        }

        let refreshed = refresh_control_clock(control, stage).await?;
        self.estimate = refreshed;
        self.next_refresh_at = next_clock_sync_refresh_at(refresh_interval);
        Ok(self.estimate)
    }
}

fn next_clock_sync_refresh_at(refresh_interval: Duration) -> Instant {
    Instant::now()
        .checked_add(refresh_interval)
        .unwrap_or_else(Instant::now)
}

async fn sync_control_clock(
    control: &crate::transport::quic::ControlClient,
    args: &Args,
) -> anyhow::Result<ClockSyncEstimate> {
    let sample_count = args.time_sync_samples.max(1);
    let sample_spacing = Duration::from_millis(args.time_sync_spacing_ms);
    let mut samples = Vec::with_capacity(sample_count as usize);

    for sample_index in 1..=sample_count {
        let client_send_time_micros = unix_time_micros();
        let response = control
            .send(ClientControl::TimeSync(TimeSyncRequest {
                client_send_time_micros,
            }))
            .await?;
        let client_receive_time_micros = unix_time_micros();
        print_control_response("time-sync", &response);
        match response.message {
            ServerControl::TimeSync(sync) => {
                let estimate = estimate_clock_sync(&sync, client_receive_time_micros);
                println!(
                    "time-sync-sample sample={} samples={} client_send_time_micros={} client_receive_time_micros={} server_receive_time_micros={} server_send_time_micros={} rtt_micros={} rtt_ms={} clock_offset_micros={}",
                    sample_index,
                    sample_count,
                    sync.client_send_time_micros,
                    client_receive_time_micros,
                    sync.server_receive_time_micros,
                    sync.server_send_time_micros,
                    estimate.rtt_micros,
                    estimate.rtt_ms,
                    estimate.clock_offset_micros
                );
                samples.push((sample_index, estimate));
            }
            ServerControl::Error(error) => bail!("time sync failed: {}", error.message),
            other => bail!("unexpected time sync response: {other:?}"),
        }
        if sample_index < sample_count && sample_spacing > Duration::ZERO {
            tokio::time::sleep(sample_spacing).await;
        }
    }

    let (selected_sample, estimate) =
        select_best_clock_sync_estimate(&samples).context("time sync produced no samples")?;
    println!(
        "time-sync selected_sample={} samples={} rtt_micros={} rtt_ms={} clock_offset_micros={}",
        selected_sample,
        sample_count,
        estimate.rtt_micros,
        estimate.rtt_ms,
        estimate.clock_offset_micros
    );
    Ok(estimate)
}

async fn refresh_control_clock(
    control: &crate::transport::quic::ControlClient,
    stage: &str,
) -> anyhow::Result<ClockSyncEstimate> {
    let client_send_time_micros = unix_time_micros();
    let response = control
        .send(ClientControl::TimeSync(TimeSyncRequest {
            client_send_time_micros,
        }))
        .await?;
    let client_receive_time_micros = unix_time_micros();
    print_control_response("time-sync-refresh", &response);
    match response.message {
        ServerControl::TimeSync(sync) => {
            let estimate = estimate_clock_sync(&sync, client_receive_time_micros);
            println!(
                "time-sync-refresh stage={} client_send_time_micros={} client_receive_time_micros={} server_receive_time_micros={} server_send_time_micros={} rtt_micros={} rtt_ms={} clock_offset_micros={}",
                stage,
                sync.client_send_time_micros,
                client_receive_time_micros,
                sync.server_receive_time_micros,
                sync.server_send_time_micros,
                estimate.rtt_micros,
                estimate.rtt_ms,
                estimate.clock_offset_micros
            );
            Ok(estimate)
        }
        ServerControl::Error(error) => bail!("time sync refresh failed: {}", error.message),
        other => bail!("unexpected time sync refresh response: {other:?}"),
    }
}

fn estimate_clock_sync(
    sync: &TimeSyncResponse,
    client_receive_time_micros: u64,
) -> ClockSyncEstimate {
    let client_elapsed_micros =
        client_receive_time_micros.saturating_sub(sync.client_send_time_micros);
    let server_processing_micros = sync
        .server_send_time_micros
        .saturating_sub(sync.server_receive_time_micros);
    let rtt_micros = client_elapsed_micros.saturating_sub(server_processing_micros);

    let client_midpoint =
        sync.client_send_time_micros as i128 + (client_elapsed_micros / 2) as i128;
    let server_midpoint =
        sync.server_receive_time_micros as i128 + (server_processing_micros / 2) as i128;
    let clock_offset_micros =
        (server_midpoint - client_midpoint).clamp(i64::MIN as i128, i64::MAX as i128) as i64;

    ClockSyncEstimate {
        rtt_micros,
        rtt_ms: micros_to_millis(rtt_micros),
        clock_offset_micros,
    }
}

fn select_best_clock_sync_estimate(
    samples: &[(u8, ClockSyncEstimate)],
) -> Option<(u8, ClockSyncEstimate)> {
    samples
        .iter()
        .copied()
        .min_by_key(|(_, estimate)| estimate.rtt_micros)
}

async fn authenticate_control(
    control: &crate::transport::quic::ControlClient,
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

async fn set_voice_state_if_requested(
    control: &crate::transport::quic::ControlClient,
    args: &Args,
    room_id: RoomId,
) -> anyhow::Result<()> {
    if !args.voice_state_requested() {
        return Ok(());
    }
    let response = control
        .send(ClientControl::SetVoiceState(SetVoiceState {
            room_id,
            muted: args.muted,
            deafened: args.deafened,
            push_to_talk: args.voice_push_to_talk_enabled(),
            speaking: args.voice_speaking(),
        }))
        .await?;
    print_control_response("set-voice-state", &response);
    match response.message {
        ServerControl::VoiceStateUpdated(state) => {
            println!(
                "voice-state room_id={} user_id={} muted={} deafened={} push_to_talk={} speaking={}",
                state.room_id,
                state.user_id,
                state.muted,
                state.deafened,
                state.push_to_talk,
                state.speaking
            );
            Ok(())
        }
        ServerControl::Error(error) => bail!("set voice state failed: {}", error.message),
        other => bail!("unexpected set voice state response: {other:?}"),
    }
}

async fn run_list_rooms_flow(
    control: &crate::transport::quic::ControlClient,
) -> anyhow::Result<()> {
    let rooms = list_rooms(control).await?;
    println!("rooms count={}", rooms.len());
    for room in &rooms {
        println!("{}", format_room_summary(room));
    }
    Ok(())
}

async fn run_list_streams_flow(
    control: &crate::transport::quic::ControlClient,
    args: &Args,
) -> anyhow::Result<()> {
    let room_id = resolve_viewer_room_id(control, args).await?;
    let joined = control
        .send(ClientControl::JoinRoom(JoinRoom { room_id }))
        .await?;
    print_control_response("join-room", &joined);
    ensure_not_error("join room", &joined)?;

    let streams = list_room_streams(control, room_id).await?;
    println!("streams room_id={} count={}", room_id, streams.len());
    for stream in &streams {
        println!("{}", format_stream_summary(stream));
    }
    leave_room(control, room_id).await?;
    Ok(())
}

async fn run_list_participants_flow(
    control: &crate::transport::quic::ControlClient,
    args: &Args,
) -> anyhow::Result<()> {
    let room_id = resolve_viewer_room_id(control, args).await?;
    let joined = control
        .send(ClientControl::JoinRoom(JoinRoom { room_id }))
        .await?;
    print_control_response("join-room", &joined);
    ensure_not_error("join room", &joined)?;
    set_voice_state_if_requested(control, args, room_id).await?;

    let participants = list_room_participants(control, room_id).await?;
    println!(
        "participants room_id={} count={}",
        room_id,
        participants.len()
    );
    for participant in &participants {
        println!("{}", format_participant_summary(participant));
    }
    leave_room(control, room_id).await?;
    Ok(())
}

async fn run_broadcaster_control_flow(
    control: &crate::transport::quic::ControlClient,
    args: &Args,
    clock_sync: ClockSyncEstimate,
) -> anyhow::Result<()> {
    let room_id = match args.selected_room_id()? {
        Some(room_id) => room_id,
        None => {
            let response = control
                .send(ClientControl::CreateRoom(CreateRoom {
                    name: args.selected_channel_name()?.to_owned(),
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
    set_voice_state_if_requested(control, args, room_id).await?;

    if args.media_kind == MediaKindArg::Both {
        run_broadcaster_dual_stream_control_flow(control, args, room_id, clock_sync).await?;
        leave_room(control, room_id).await?;
        return Ok(());
    }

    publish_configured_stream(control, args, room_id).await?;

    println!(
        "control-flow broadcaster room_id={} stream_id={}",
        room_id, args.stream_id
    );
    if args.synthetic_media_enabled() {
        set_publisher_target_media(control, room_id, args).await?;
        run_synthetic_broadcaster_media(control, args, room_id, clock_sync).await?;
    }
    leave_room(control, room_id).await?;
    Ok(())
}

async fn publish_configured_stream(
    control: &crate::transport::quic::ControlClient,
    args: &Args,
    room_id: RoomId,
) -> anyhow::Result<()> {
    let published = control
        .send(ClientControl::PublishStream(PublishStream {
            room_id,
            stream_id: args.stream_id,
            codec: args.codec()?,
            media_kind: args.protocol_media_kind()?,
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
    Ok(())
}

async fn run_broadcaster_dual_stream_control_flow(
    control: &crate::transport::quic::ControlClient,
    args: &Args,
    room_id: RoomId,
    clock_sync: ClockSyncEstimate,
) -> anyhow::Result<()> {
    let voice_stream_id = args.voice_stream_id()?;
    let screen_args = args.for_media_kind(MediaKindArg::Screen, args.stream_id);
    let voice_args = args.for_media_kind(MediaKindArg::Voice, voice_stream_id);

    publish_configured_stream(control, &screen_args, room_id).await?;
    publish_configured_stream(control, &voice_args, room_id).await?;
    println!(
        "control-flow broadcaster room_id={} screen_stream_id={} voice_stream_id={}",
        room_id, screen_args.stream_id, voice_args.stream_id
    );
    if args.synthetic_media_enabled() {
        set_publisher_target_media(control, room_id, &screen_args).await?;
        set_publisher_target_media(control, room_id, &voice_args).await?;
        let screen_control = control.clone();
        let voice_control = control.clone();
        tokio::try_join!(
            run_synthetic_screen_broadcaster_media(
                &screen_control,
                &screen_args,
                room_id,
                clock_sync
            ),
            run_synthetic_voice_broadcaster_media(&voice_control, &voice_args, room_id, clock_sync)
        )?;
    }
    Ok(())
}

async fn run_viewer_control_flow(
    control: &crate::transport::quic::ControlClient,
    args: &Args,
    clock_sync: ClockSyncEstimate,
) -> anyhow::Result<()> {
    let room_id = resolve_viewer_room_id(control, args).await?;

    let joined = control
        .send(ClientControl::JoinRoom(JoinRoom { room_id }))
        .await?;
    print_control_response("join-room", &joined);
    ensure_not_error("join room", &joined)?;
    set_voice_state_if_requested(control, args, room_id).await?;

    let available_streams = list_room_streams(control, room_id).await?;
    if args.media_kind == MediaKindArg::Both {
        run_dual_stream_viewer_control_flow(control, args, room_id, &available_streams, clock_sync)
            .await?;
        leave_room(control, room_id).await?;
        return Ok(());
    }

    let stream_id = resolve_viewer_stream_id(args, &available_streams)?;

    subscribe_stream(control, room_id, stream_id).await?;

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
    send_remote_input_script_if_requested(control, args, room_id, stream_id).await?;

    println!(
        "control-flow viewer room_id={} stream_id={}",
        room_id, stream_id
    );
    if args.synthetic_media_enabled() {
        run_synthetic_viewer_media(control, args, room_id, stream_id, clock_sync).await?;
    }
    unsubscribe_stream(control, room_id, stream_id).await?;
    leave_room(control, room_id).await?;
    Ok(())
}

async fn run_dual_stream_viewer_control_flow(
    control: &crate::transport::quic::ControlClient,
    args: &Args,
    room_id: RoomId,
    available_streams: &[StreamSummary],
    clock_sync: ClockSyncEstimate,
) -> anyhow::Result<()> {
    let (screen_stream_id, voice_stream_id) =
        resolve_dual_viewer_stream_ids(args, available_streams)?;

    subscribe_stream(control, room_id, screen_stream_id).await?;
    subscribe_stream(control, room_id, voice_stream_id).await?;

    let screen_config = poll_stream_config(control, room_id, screen_stream_id).await?;
    let voice_config = poll_stream_config(control, room_id, voice_stream_id).await?;
    println!(
        "stream-config stream_id={} codec={:?} width={} height={} fps={} timebase_hz={}",
        screen_config.stream_id,
        screen_config.codec,
        screen_config.width,
        screen_config.height,
        screen_config.frames_per_second,
        screen_config.timebase_hz
    );
    println!(
        "stream-config stream_id={} codec={:?} width={} height={} fps={} timebase_hz={}",
        voice_config.stream_id,
        voice_config.codec,
        voice_config.width,
        voice_config.height,
        voice_config.frames_per_second,
        voice_config.timebase_hz
    );
    send_remote_input_script_if_requested(control, args, room_id, screen_stream_id).await?;

    println!(
        "control-flow viewer room_id={} screen_stream_id={} voice_stream_id={}",
        room_id, screen_stream_id, voice_stream_id
    );
    if args.synthetic_media_enabled() {
        run_synthetic_dual_viewer_media(
            control,
            args,
            room_id,
            screen_stream_id,
            voice_stream_id,
            clock_sync,
        )
        .await?;
    }
    unsubscribe_stream(control, room_id, screen_stream_id).await?;
    unsubscribe_stream(control, room_id, voice_stream_id).await?;
    Ok(())
}

async fn resolve_viewer_room_id(
    control: &crate::transport::quic::ControlClient,
    args: &Args,
) -> anyhow::Result<RoomId> {
    if let Some(room_id) = args.selected_room_id()? {
        return Ok(room_id);
    }

    let rooms = list_rooms(control).await?;
    if rooms.is_empty() {
        bail!("no channels are available; start a broadcaster first or pass --channel-id");
    }
    let channel_name = args.selected_channel_name()?;
    let selected = select_viewer_room(&rooms, channel_name).with_context(|| {
        format!(
            "no channel matched name {:?}; available channels: {}",
            channel_name,
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
    control: &crate::transport::quic::ControlClient,
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
    control: &crate::transport::quic::ControlClient,
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

async fn list_room_participants(
    control: &crate::transport::quic::ControlClient,
    room_id: RoomId,
) -> anyhow::Result<Vec<ParticipantSummary>> {
    let response = control
        .send(ClientControl::ListParticipants(ListParticipants {
            room_id,
        }))
        .await?;
    print_control_response("list-participants", &response);
    match response.message {
        ServerControl::ParticipantList(list) => Ok(list.participants),
        ServerControl::Error(error) => bail!("list participants failed: {}", error.message),
        other => bail!("unexpected list participants response: {other:?}"),
    }
}

fn format_participant_summary(participant: &ParticipantSummary) -> String {
    format!(
        "participant room_id={} user_id={} display_name={:?} muted={} deafened={} push_to_talk={} speaking={} published_streams={} subscribed_streams={}",
        participant.room_id,
        participant.user_id,
        participant.display_name,
        participant.muted,
        participant.deafened,
        participant.push_to_talk,
        participant.speaking,
        participant.published_stream_count,
        participant.subscribed_stream_count
    )
}

fn format_room_summary(room: &RoomSummary) -> String {
    format!(
        "room room_id={} name={:?} participants={} streams={}",
        room.room_id, room.name, room.participant_count, room.published_stream_count
    )
}

fn format_stream_summary(stream: &StreamSummary) -> String {
    format!(
        "stream room_id={} stream_id={} publisher_id={} codec={:?} media_kind={:?} subscribers={} configured={} target_bitrate_bps={} target_fps={}",
        stream.room_id,
        stream.stream_id,
        stream.publisher_id,
        stream.codec,
        stream.media_kind,
        stream.subscriber_count,
        stream.has_config,
        stream.target_bitrate_bps,
        stream.target_frames_per_second
    )
}

fn format_remote_input_event(event: &RemoteInputEvent) -> String {
    match &event.kind {
        RemoteInputKind::PointerMove {
            normalized_x,
            normalized_y,
        } => format!(
            "remote-input event=pointer-move sender_user_id={} sequence={} event_time_micros={} x={} y={}",
            event.sender_user_id,
            event.sequence_number,
            event.event_time_micros,
            normalized_x,
            normalized_y
        ),
        RemoteInputKind::PointerButton {
            button,
            pressed,
            normalized_x,
            normalized_y,
        } => format!(
            "remote-input event=pointer-button sender_user_id={} sequence={} event_time_micros={} button={:?} pressed={} x={} y={}",
            event.sender_user_id,
            event.sequence_number,
            event.event_time_micros,
            button,
            pressed,
            normalized_x,
            normalized_y
        ),
        RemoteInputKind::PointerWheel {
            delta_x,
            delta_y,
            normalized_x,
            normalized_y,
        } => format!(
            "remote-input event=pointer-wheel sender_user_id={} sequence={} event_time_micros={} delta_x={} delta_y={} x={} y={}",
            event.sender_user_id,
            event.sequence_number,
            event.event_time_micros,
            delta_x,
            delta_y,
            normalized_x,
            normalized_y
        ),
        RemoteInputKind::Key { key_code, pressed } => format!(
            "remote-input event=key sender_user_id={} sequence={} event_time_micros={} key_code={} pressed={}",
            event.sender_user_id, event.sequence_number, event.event_time_micros, key_code, pressed
        ),
        RemoteInputKind::Text { text } => format!(
            "remote-input event=text sender_user_id={} sequence={} event_time_micros={} text={:?}",
            event.sender_user_id, event.sequence_number, event.event_time_micros, text
        ),
    }
}

fn resolve_viewer_stream_id(args: &Args, streams: &[StreamSummary]) -> anyhow::Result<StreamId> {
    let expected_kind = args.protocol_media_kind()?;
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

fn resolve_dual_viewer_stream_ids(
    args: &Args,
    streams: &[StreamSummary],
) -> anyhow::Result<(StreamId, StreamId)> {
    let screen_stream_id = resolve_stream_id_for_kind(args.stream_id, MediaKind::Screen, streams)?
        .ok_or_else(|| {
            anyhow::anyhow!(
                "screen stream {} was not found; streams: {}",
                args.stream_id,
                format_stream_summaries(streams)
            )
        })?;
    let requested_voice_stream_id = args.voice_stream_id()?;
    let voice_stream_id =
        resolve_stream_id_for_kind(requested_voice_stream_id, MediaKind::Voice, streams)?
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "voice stream {} was not found; streams: {}",
                    requested_voice_stream_id,
                    format_stream_summaries(streams)
                )
            })?;
    Ok((screen_stream_id, voice_stream_id))
}

fn resolve_stream_id_for_kind(
    requested_stream_id: StreamId,
    expected_kind: MediaKind,
    streams: &[StreamSummary],
) -> anyhow::Result<Option<StreamId>> {
    let Some(stream) = streams
        .iter()
        .find(|stream| stream.stream_id == requested_stream_id)
    else {
        return Ok(None);
    };
    if stream.media_kind != expected_kind {
        bail!(
            "stream {} is {:?}, but viewer requested {:?}",
            requested_stream_id,
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
    Ok(Some(stream.stream_id))
}

async fn subscribe_stream(
    control: &crate::transport::quic::ControlClient,
    room_id: RoomId,
    stream_id: StreamId,
) -> anyhow::Result<()> {
    let subscribed = control
        .send(ClientControl::SubscribeStream(SubscribeStream {
            room_id,
            stream_id,
        }))
        .await?;
    print_control_response("subscribe-stream", &subscribed);
    ensure_not_error("subscribe stream", &subscribed)
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
    control: &crate::transport::quic::ControlClient,
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
    control: &crate::transport::quic::ControlClient,
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
    control: &crate::transport::quic::ControlClient,
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
    control: &crate::transport::quic::ControlClient,
    args: &Args,
    room_id: RoomId,
    clock_sync: ClockSyncEstimate,
) -> anyhow::Result<()> {
    match args.media_kind {
        MediaKindArg::Screen => {
            run_synthetic_screen_broadcaster_media(control, args, room_id, clock_sync).await
        }
        MediaKindArg::Voice => {
            run_synthetic_voice_broadcaster_media(control, args, room_id, clock_sync).await
        }
        MediaKindArg::Both => {
            bail!("dual-stream broadcaster media should be started by control flow")
        }
    }
}

async fn run_synthetic_screen_broadcaster_media(
    control: &crate::transport::quic::ControlClient,
    args: &Args,
    room_id: RoomId,
    clock_sync: ClockSyncEstimate,
) -> anyhow::Result<()> {
    if args.media_start_delay_ms > 0 {
        sleep_with_keepalive(control, Duration::from_millis(args.media_start_delay_ms)).await?;
    }
    let media_frames = args.synthetic_media_frames()?;
    let frame_interval = args.media_frame_interval()?;
    let (mut target_width, mut target_height) = args.screen_capture_dimensions()?;

    let mut capture =
        windows::WindowsGraphicsCapture::new(args.capture_source()?, args.capture_config())?;
    let mut encoder = args.video_encoder(target_width, target_height)?;
    encoder.request_keyframe();
    let mut remote_input = args.remote_input_applier()?;
    println!("remote-input-output mode={}", remote_input.output_mode());

    let mut ticker = tokio::time::interval(frame_interval);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut active_fps = args.media_fps;
    let mut sent_packets = 0_u64;
    let mut next_sequence_number = 1_u32;
    let mut timing = ClientBroadcasterStats::default();
    let mut clock_sync = ClockSyncTracker::new(clock_sync, args.clock_sync_refresh_interval());
    for frame_id in 1..=media_frames {
        ticker.tick().await;
        let capture_start = Instant::now();
        let Some(captured) = capture_screen_frame(&mut capture, args, target_width, target_height)?
        else {
            continue;
        };
        let capture_duration = capture_start.elapsed();
        timing.record_capture_duration(capture_duration);
        let captured_width = captured.width;
        let captured_height = captured.height;
        let encode_start = Instant::now();
        let Some(mut frame) = encoder.encode(captured, args.stream_id)? else {
            timing.record_encode_duration(encode_start.elapsed());
            continue;
        };
        let sync_estimate = clock_sync
            .refresh_if_due(control, "screen-broadcaster")
            .await?;
        frame.sender_clock_offset_micros = sync_estimate.clock_offset_micros;
        frame.sender_encode_done_time_micros = unix_time_micros();
        let encode_duration = encode_start.elapsed();
        timing.record_encode_duration(encode_duration);
        let packetize_start = Instant::now();
        let mut packets = packetize_frame_for_datagram_target(
            &frame,
            next_sequence_number,
            args.max_datagram_payload,
        )?;
        let packetize_duration = packetize_start.elapsed();
        timing.record_packetize_duration(packetize_duration);
        next_sequence_number = next_sequence_number.wrapping_add(packets.len() as u32);
        let send_start = Instant::now();
        for packet in &mut packets {
            packet.header.sender_send_time_micros = unix_time_micros();
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
            let resolution_changed = apply_publisher_feedback(
                &feedback,
                &mut encoder,
                &mut active_fps,
                &mut target_width,
                &mut target_height,
                &mut ticker,
            );
            if resolution_changed {
                set_screen_stream_config(
                    control,
                    room_id,
                    args,
                    target_width,
                    target_height,
                    active_fps,
                )
                .await?;
            }
            poll_remote_input(control, room_id, args.stream_id, remote_input.as_mut()).await?;
        }
    }
    let feedback = poll_publisher_feedback(control, room_id, args.stream_id).await?;
    if feedback.keyframe_requested {
        println!(
            "publisher-feedback stream_id={} keyframe_requested=true degraded_viewers={} total_viewers={}",
            feedback.stream_id, feedback.degraded_viewer_count, feedback.total_viewer_count
        );
    }
    let resolution_changed = apply_publisher_feedback(
        &feedback,
        &mut encoder,
        &mut active_fps,
        &mut target_width,
        &mut target_height,
        &mut ticker,
    );
    if resolution_changed {
        set_screen_stream_config(
            control,
            room_id,
            args,
            target_width,
            target_height,
            active_fps,
        )
        .await?;
    }
    poll_remote_input(control, room_id, args.stream_id, remote_input.as_mut()).await?;
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
    println!(
        "remote-input-summary output={} applied={}",
        remote_input.output_mode(),
        remote_input.applied_events()
    );
    if args.media_end_linger_ms > 0 {
        tokio::time::sleep(Duration::from_millis(args.media_end_linger_ms)).await;
    }
    Ok(())
}

fn capture_screen_frame(
    capture: &mut windows::WindowsGraphicsCapture,
    args: &Args,
    target_width: u32,
    target_height: u32,
) -> anyhow::Result<Option<capture::CaptureFrame>> {
    match args.screen_input {
        ScreenInputArg::Synthetic => {
            capture.push_test_frame(
                target_width.max(1),
                target_height.max(1),
                unix_time_micros(),
            );
            capture.next_frame()
        }
        ScreenInputArg::Live => capture
            .next_frame()?
            .map(|frame| resize_capture_frame(frame, target_width, target_height))
            .transpose(),
    }
}

fn resize_capture_frame(
    frame: capture::CaptureFrame,
    target_width: u32,
    target_height: u32,
) -> anyhow::Result<capture::CaptureFrame> {
    if target_width == 0 || target_height == 0 {
        return Ok(frame);
    }
    frame.resize_bgra_nearest(target_width, target_height)
}

async fn run_synthetic_voice_broadcaster_media(
    control: &crate::transport::quic::ControlClient,
    args: &Args,
    room_id: RoomId,
    clock_sync: ClockSyncEstimate,
) -> anyhow::Result<()> {
    if !args.voice_speaking() {
        println!(
            "voice-send-disabled role=broadcaster room_id={} stream_id={} muted={} push_to_talk={} speaking={}",
            room_id,
            args.stream_id,
            args.muted,
            args.voice_push_to_talk_enabled(),
            args.voice_speaking()
        );
        println!(
            "media-summary role=broadcaster kind=voice inactive=true frames=0 packets=0 fps=0 run_ms=0 capture_ms_p50=0 capture_ms_p95=0 encode_ms_p50=0 encode_ms_p95=0 packetize_ms_p50=0 packetize_ms_p95=0 send_ms_p50=0 send_ms_p95=0"
        );
        return Ok(());
    }
    if args.media_start_delay_ms > 0 {
        sleep_with_keepalive(control, Duration::from_millis(args.media_start_delay_ms)).await?;
    }
    let media_frames = args.synthetic_media_frames()?;
    let frame_interval = args.media_frame_interval()?;
    let mut encoder = SyntheticOpusEncoder::default();
    encoder.config.synthetic_payload_bytes = args.active_media_frame_bytes();
    encoder.config.bitrate_bps = args.synthetic_bitrate_bps();
    encoder.set_frames_per_second(args.active_media_fps().max(1));
    let mut microphone_capture = match args.voice_input {
        VoiceInputArg::Synthetic => None,
        VoiceInputArg::Microphone => {
            let capture = WindowsMicrophoneCapture::open(
                args.microphone_source()?,
                args.audio_capture_config(),
            )
            .context("failed to open microphone capture")?;
            let capture_config = capture.config();
            encoder.config.sample_rate_hz = capture_config.sample_rate_hz;
            encoder.config.channel_count = capture_config.channel_count;
            encoder.config.frame_duration_ms = capture_config.frame_duration_ms;
            Some(capture)
        }
    };

    let mut ticker = tokio::time::interval(frame_interval);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut active_fps = args.active_media_fps();
    let mut sent_frames = 0_u32;
    let mut sent_packets = 0_u64;
    let mut next_sequence_number = 1_u32;
    let mut empty_capture_ticks = 0_u32;
    let mut timing = ClientBroadcasterStats::default();
    let mut clock_sync = ClockSyncTracker::new(clock_sync, args.clock_sync_refresh_interval());
    while sent_frames < media_frames {
        ticker.tick().await;
        let frame_id = sent_frames.saturating_add(1);
        let capture_start = Instant::now();
        let captured_audio = if let Some(capture) = microphone_capture.as_mut() {
            match capture.next_frame()? {
                Some(frame) => Some(frame),
                None => {
                    empty_capture_ticks = empty_capture_ticks.saturating_add(1);
                    if empty_capture_ticks > active_fps.max(1) as u32 {
                        bail!(
                            "microphone capture produced no audio frames for {} ticks",
                            empty_capture_ticks
                        );
                    }
                    continue;
                }
            }
        } else {
            None
        };
        empty_capture_ticks = 0;
        let capture_time_micros = captured_audio
            .as_ref()
            .map(|frame| frame.capture_time_micros)
            .unwrap_or_else(unix_time_micros);
        let capture_duration = capture_start.elapsed();
        timing.record_capture_duration(capture_duration);
        let encode_start = Instant::now();
        let mut frame = match captured_audio {
            Some(audio) => encoder.encode_pcm_i16(
                audio.frame_id.min(u32::MAX as u64) as u32,
                audio.capture_time_micros,
                audio.sample_rate_hz,
                audio.channel_count,
                &audio.samples,
                args.stream_id,
            )?,
            None => encoder.encode(frame_id, capture_time_micros, args.stream_id)?,
        };
        let sync_estimate = clock_sync
            .refresh_if_due(control, "voice-broadcaster")
            .await?;
        frame.sender_clock_offset_micros = sync_estimate.clock_offset_micros;
        frame.sender_encode_done_time_micros = unix_time_micros();
        let encode_duration = encode_start.elapsed();
        timing.record_encode_duration(encode_duration);
        let packetize_start = Instant::now();
        let mut packets = packetize_frame_with_type_for_datagram_target(
            &frame,
            PacketType::Audio,
            next_sequence_number,
            args.max_datagram_payload,
        )?;
        let packetize_duration = packetize_start.elapsed();
        timing.record_packetize_duration(packetize_duration);
        next_sequence_number = next_sequence_number.wrapping_add(packets.len() as u32);
        let send_start = Instant::now();
        for packet in &mut packets {
            packet.header.sender_send_time_micros = unix_time_micros();
            control.send_media_packet(packet)?;
            sent_packets += 1;
        }
        let send_duration = send_start.elapsed();
        timing.record_send_duration(send_duration);
        sent_frames = sent_frames.saturating_add(1);
        println!(
            "audio-send frame_id={} fragments={} bytes={} target_bytes={} voice_input={:?} capture_ms={} encode_ms={} packetize_ms={} send_ms={}",
            frame.frame_id,
            packets.len(),
            frame.bytes.len(),
            encoder.config.synthetic_payload_bytes,
            args.voice_input,
            millis_for_log(capture_duration),
            millis_for_log(encode_duration),
            millis_for_log(packetize_duration),
            millis_for_log(send_duration)
        );
        if args.feedback_interval_frames > 0
            && sent_frames.is_multiple_of(args.feedback_interval_frames)
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
        sent_frames,
        sent_packets,
        args.active_media_fps(),
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
    control: &crate::transport::quic::ControlClient,
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
            frames_per_second: args.active_media_fps().max(1),
        }))
        .await?;
    print_control_response("set-target-framerate", &framerate);
    ensure_not_error("set target framerate", &framerate)?;
    Ok(())
}

async fn set_screen_stream_config(
    control: &crate::transport::quic::ControlClient,
    room_id: RoomId,
    args: &Args,
    width: u32,
    height: u32,
    frames_per_second: u16,
) -> anyhow::Result<()> {
    let config = StreamConfig {
        room_id,
        stream_id: args.stream_id,
        codec: CodecId::H264,
        width,
        height,
        frames_per_second,
        timebase_hz: 90_000,
    };
    let response = control
        .send(ClientControl::SetStreamConfig(config.clone()))
        .await?;
    print_control_response("set-stream-config", &response);
    match response.message {
        ServerControl::StreamConfig(returned) if returned == config => Ok(()),
        ServerControl::Error(error) => bail!("set stream config failed: {}", error.message),
        other => bail!("unexpected stream config response: {other:?}"),
    }
}

fn apply_publisher_feedback(
    feedback: &PublisherFeedback,
    encoder: &mut dyn VideoEncoder,
    active_fps: &mut u16,
    target_width: &mut u32,
    target_height: &mut u32,
    ticker: &mut tokio::time::Interval,
) -> bool {
    let mut adapted = false;
    let mut resolution_changed = false;
    let target_bitrate = feedback.aggregate_available_bitrate_bps;
    if target_bitrate > 0 && target_bitrate != encoder.bitrate_bps() {
        encoder.update_bitrate(target_bitrate);
        encoder.set_target_payload_bytes(synthetic_payload_bytes_for_bitrate(
            target_bitrate,
            (*active_fps).max(1),
        ));
        adapted = true;
    }
    if feedback.target_frames_per_second > 0 && feedback.target_frames_per_second != *active_fps {
        *active_fps = feedback.target_frames_per_second;
        encoder.update_frame_rate(*active_fps);
        encoder.set_target_payload_bytes(synthetic_payload_bytes_for_bitrate(
            encoder.bitrate_bps(),
            (*active_fps).max(1),
        ));
        *ticker = tokio::time::interval(Duration::from_micros(media_interval_micros(*active_fps)));
        ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
        adapted = true;
    }
    if feedback.target_width > 0
        && feedback.target_height > 0
        && (feedback.target_width != *target_width || feedback.target_height != *target_height)
    {
        *target_width = feedback.target_width;
        *target_height = feedback.target_height;
        encoder.update_resolution(*target_width, *target_height);
        encoder.request_keyframe();
        adapted = true;
        resolution_changed = true;
    }
    if adapted {
        println!(
            "publisher-adapt stream_id={} bitrate_bps={} fps={} width={} height={} frame_bytes={}",
            feedback.stream_id,
            encoder.bitrate_bps(),
            *active_fps,
            *target_width,
            *target_height,
            encoder.target_payload_bytes()
        );
    }
    resolution_changed
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

fn audio_frame_duration_ms(frames_per_second: u16) -> u16 {
    (1_000 / frames_per_second.max(1)).max(1)
}

fn millis_for_log(duration: Duration) -> u16 {
    duration.as_millis().min(u16::MAX as u128) as u16
}

fn micros_to_millis(micros: u64) -> u16 {
    micros.saturating_div(1_000).min(u16::MAX as u64) as u16
}

fn unix_time_micros() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_micros()
        .min(u64::MAX as u128) as u64
}

async fn poll_publisher_feedback(
    control: &crate::transport::quic::ControlClient,
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
    control: &crate::transport::quic::ControlClient,
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

async fn send_remote_input_script_if_requested(
    control: &crate::transport::quic::ControlClient,
    args: &Args,
    room_id: RoomId,
    stream_id: StreamId,
) -> anyhow::Result<()> {
    let Some(script) = args.remote_input_script else {
        return Ok(());
    };
    if args.media_kind == MediaKindArg::Voice {
        bail!("--remote-input-script requires a screen stream");
    }

    for (index, kind) in args
        .remote_input_script_events(script)
        .into_iter()
        .enumerate()
    {
        let sequence_number = index as u64 + 1;
        let response = control
            .send(ClientControl::SendRemoteInput(SendRemoteInput {
                room_id,
                stream_id,
                sequence_number,
                event_time_micros: unix_time_micros(),
                kind,
            }))
            .await?;
        print_control_response("remote-input-send", &response);
        match response.message {
            ServerControl::RemoteInputQueued(queued) => {
                println!(
                    "remote-input-send stream_id={} sequence={} queued={} dropped={} publisher_id={}",
                    queued.stream_id,
                    sequence_number,
                    queued.queued_events,
                    queued.dropped_events,
                    queued.publisher_id
                );
            }
            ServerControl::Error(error) => bail!("send remote input failed: {}", error.message),
            other => bail!("unexpected remote input response: {other:?}"),
        }
    }
    Ok(())
}

async fn poll_remote_input(
    control: &crate::transport::quic::ControlClient,
    room_id: RoomId,
    stream_id: StreamId,
    applier: &mut dyn RemoteInputApplier,
) -> anyhow::Result<()> {
    let response = control
        .send(ClientControl::PollRemoteInput(PollRemoteInput {
            room_id,
            stream_id,
            max_events: 64,
        }))
        .await?;
    print_control_response("remote-input-poll", &response);
    match response.message {
        ServerControl::RemoteInputBatch(batch) => {
            println!(
                "remote-input-batch stream_id={} events={}",
                batch.stream_id,
                batch.events.len()
            );
            for event in &batch.events {
                println!("{}", format_remote_input_event(event));
                applier.apply(event)?;
            }
            Ok(())
        }
        ServerControl::Error(error) => bail!("poll remote input failed: {}", error.message),
        other => bail!("unexpected remote input poll response: {other:?}"),
    }
}

async fn run_synthetic_viewer_media(
    control: &crate::transport::quic::ControlClient,
    args: &Args,
    room_id: RoomId,
    stream_id: StreamId,
    clock_sync: ClockSyncEstimate,
) -> anyhow::Result<()> {
    match args.media_kind {
        MediaKindArg::Screen => {
            run_synthetic_screen_viewer_media(control, args, room_id, stream_id, clock_sync).await
        }
        MediaKindArg::Voice => {
            run_synthetic_voice_viewer_media(control, args, room_id, stream_id, clock_sync).await
        }
        MediaKindArg::Both => bail!("viewer --media-kind both is not supported yet"),
    }
}

async fn run_synthetic_dual_viewer_media(
    control: &crate::transport::quic::ControlClient,
    args: &Args,
    room_id: RoomId,
    screen_stream_id: StreamId,
    voice_stream_id: StreamId,
    clock_sync: ClockSyncEstimate,
) -> anyhow::Result<()> {
    let target_screen_frames =
        args.synthetic_media_frames_for_fps(args.media_fps, "--media-fps")?;
    let target_voice_frames = if args.deafened {
        0
    } else {
        args.synthetic_media_frames_for_fps(args.voice_frames_per_second(), "--voice-fps")?
    };
    let screen_frame_interval = args.media_frame_interval_for_fps(args.media_fps, "--media-fps")?;
    let voice_frame_interval =
        args.media_frame_interval_for_fps(args.voice_frames_per_second(), "--voice-fps")?;
    let screen_frame_interval_ms = screen_frame_interval.as_millis().min(u16::MAX as u128) as u16;
    let voice_frame_interval_ms = voice_frame_interval.as_millis().min(u16::MAX as u128) as u16;
    let mut clock_sync = ClockSyncTracker::new(clock_sync, args.clock_sync_refresh_interval());

    let mut screen_buffer = args.frame_reassembly_buffer(screen_frame_interval_ms);
    let mut screen_decoder = args.video_decoder()?;
    let mut screen_playback = args.video_playback()?;
    let mut screen_stats = ClientMediaStats::default();
    let mut screen_reassembled_frames = 0_u32;
    let mut screen_decoded_frames = 0_u32;
    let mut screen_received_packets = 0_u64;
    let mut awaiting_recovery_keyframe = false;

    let mut voice_buffer = args.frame_reassembly_buffer(voice_frame_interval_ms);
    let mut voice_decoder = SyntheticOpusDecoder;
    let mut voice_playback = (!args.deafened)
        .then(|| args.audio_playback())
        .transpose()?;
    let mut voice_stats = ClientMediaStats::default();
    let mut voice_reassembled_frames = 0_u32;
    let mut voice_decoded_frames = 0_u32;
    let mut voice_received_packets = 0_u64;
    if args.deafened {
        println!(
            "voice-deafened role=viewer room_id={} stream_id={}",
            room_id, voice_stream_id
        );
    }

    while screen_reassembled_frames < target_screen_frames
        || voice_reassembled_frames < target_voice_frames
    {
        let packet = match recv_media_packet_with_keepalive(
            control,
            Duration::from_millis(args.media_idle_timeout_ms),
        )
        .await?
        {
            Some(packet) => packet,
            None => {
                if args.media_frames == 0
                    && (screen_received_packets > 0 || voice_received_packets > 0)
                {
                    break;
                }
                bail!("timed out waiting for dual-stream media packet");
            }
        };
        let packet_receive_time_micros = unix_time_micros();
        let sync_estimate = clock_sync.refresh_if_due(control, "dual-viewer").await?;
        let packet_stream_id = packet.header.room_stream_id;

        if packet_stream_id == screen_stream_id && screen_reassembled_frames < target_screen_frames
        {
            let lost_packets_before = screen_stats.lost_packets;
            screen_stats.record_packet(&packet);
            screen_received_packets += 1;
            let outcome = screen_buffer.push_with_stats_at(packet, packet_receive_time_micros)?;
            let packet_loss_detected = screen_stats.lost_packets > lost_packets_before;
            if outcome.dropped_frames > 0 {
                screen_stats.record_dropped_frames(outcome.dropped_frames);
            }
            if (packet_loss_detected || outcome.dropped_frames > 0) && !awaiting_recovery_keyframe {
                request_keyframe(
                    control,
                    room_id,
                    screen_stream_id,
                    KeyframeReason::PacketLoss,
                )
                .await?;
                awaiting_recovery_keyframe = true;
            }
            screen_stats.jitter_buffer_ms =
                screen_buffer.estimated_jitter_ms(screen_frame_interval_ms.max(1));
            if let Some(frame) = outcome.frame {
                screen_stats.record_reassembly_millis(outcome.reassembly_ms);
                screen_stats.record_estimated_latency(
                    frame.sender_capture_time_micros,
                    packet_receive_time_micros,
                );
                screen_stats.record_calibrated_latency(
                    frame.sender_capture_time_micros,
                    frame.sender_clock_offset_micros,
                    packet_receive_time_micros,
                    sync_estimate.clock_offset_micros,
                );
                screen_stats.record_sender_timestamps(
                    frame.sender_capture_time_micros,
                    frame.sender_encode_done_time_micros,
                    frame.sender_send_time_micros,
                );
                screen_stats.record_server_queue_latency(
                    frame.server_receive_time_micros,
                    frame.server_send_time_micros,
                );
                println!(
                    "media-recv frame_id={} stream_id={} bytes={} keyframe={} latency_ms={} calibrated_latency_ms={} sender_encode_ms={} sender_send_ms={} server_queue_ms={} reassembly_ms={}",
                    frame.frame_id,
                    screen_stream_id,
                    frame.bytes.len(),
                    frame.is_keyframe,
                    screen_stats.estimated_latency_ms,
                    screen_stats.calibrated_latency_ms,
                    screen_stats.sender_encode_ms,
                    screen_stats.sender_send_ms,
                    screen_stats.server_queue_ms,
                    outcome.reassembly_ms
                );
                let decode_start = Instant::now();
                if let Some(decoded) = screen_decoder.decode(&frame.bytes)? {
                    let decode_duration = decode_start.elapsed();
                    screen_stats.record_decode_duration(decode_duration);
                    let render_start = Instant::now();
                    screen_playback.render(decoded)?;
                    let render_duration = render_start.elapsed();
                    if let Some(rendered) = screen_playback.latest_frame() {
                        screen_stats
                            .record_render_duration(render_duration, rendered.render_time_micros);
                        println!(
                            "media-render frame_id={} stream_id={} width={} height={} pixel_bytes={} render_time_micros={} decode_ms={} render_ms={} render_fps={}",
                            rendered.frame_id,
                            screen_stream_id,
                            rendered.width,
                            rendered.height,
                            rendered.pixel_bytes,
                            rendered.render_time_micros,
                            millis_for_log(decode_duration),
                            millis_for_log(render_duration),
                            screen_stats.render_fps()
                        );
                    }
                    screen_stats.record_decoded_frame();
                    screen_decoded_frames += 1;
                    if frame.is_keyframe {
                        awaiting_recovery_keyframe = false;
                    }
                } else {
                    screen_stats.record_dropped_frame();
                    if !awaiting_recovery_keyframe {
                        request_keyframe(
                            control,
                            room_id,
                            screen_stream_id,
                            KeyframeReason::DecoderRecovery,
                        )
                        .await?;
                        awaiting_recovery_keyframe = true;
                    }
                }
                screen_reassembled_frames += 1;
                if args.stats_interval_frames > 0
                    && screen_reassembled_frames.is_multiple_of(args.stats_interval_frames)
                {
                    send_viewer_stats(control, room_id, screen_stream_id, screen_stats).await?;
                }
            }
        } else if packet_stream_id == voice_stream_id
            && voice_reassembled_frames < target_voice_frames
        {
            voice_stats.record_packet(&packet);
            voice_received_packets += 1;
            let outcome = voice_buffer.push_with_stats_at(packet, packet_receive_time_micros)?;
            if outcome.dropped_frames > 0 {
                voice_stats.record_dropped_frames(outcome.dropped_frames);
            }
            voice_stats.jitter_buffer_ms =
                voice_buffer.estimated_jitter_ms(voice_frame_interval_ms.max(1));
            if let Some(frame) = outcome.frame {
                voice_stats.record_reassembly_millis(outcome.reassembly_ms);
                voice_stats.record_estimated_latency(
                    frame.sender_capture_time_micros,
                    packet_receive_time_micros,
                );
                voice_stats.record_calibrated_latency(
                    frame.sender_capture_time_micros,
                    frame.sender_clock_offset_micros,
                    packet_receive_time_micros,
                    sync_estimate.clock_offset_micros,
                );
                voice_stats.record_sender_timestamps(
                    frame.sender_capture_time_micros,
                    frame.sender_encode_done_time_micros,
                    frame.sender_send_time_micros,
                );
                voice_stats.record_server_queue_latency(
                    frame.server_receive_time_micros,
                    frame.server_send_time_micros,
                );
                println!(
                    "audio-recv frame_id={} stream_id={} bytes={} latency_ms={} calibrated_latency_ms={} sender_encode_ms={} sender_send_ms={} server_queue_ms={} reassembly_ms={}",
                    frame.frame_id,
                    voice_stream_id,
                    frame.bytes.len(),
                    voice_stats.estimated_latency_ms,
                    voice_stats.calibrated_latency_ms,
                    voice_stats.sender_encode_ms,
                    voice_stats.sender_send_ms,
                    voice_stats.server_queue_ms,
                    outcome.reassembly_ms
                );
                let decode_start = Instant::now();
                if let Some(decoded) = voice_decoder.decode(&frame.bytes)? {
                    let decode_duration = decode_start.elapsed();
                    voice_stats.record_decode_duration(decode_duration);
                    let play_start = Instant::now();
                    let voice_playback = voice_playback
                        .as_mut()
                        .context("voice playback is disabled while deafened")?;
                    voice_playback.play(decoded)?;
                    let play_duration = play_start.elapsed();
                    voice_stats.record_render_duration(play_duration, unix_time_micros());
                    if let Some(played) = voice_playback.latest() {
                        println!(
                            "audio-play frame_id={} stream_id={} sample_rate_hz={} channels={} samples={} decode_ms={} play_ms={} play_fps={}",
                            played.frame_id,
                            voice_stream_id,
                            played.sample_rate_hz,
                            played.channel_count,
                            played.sample_count,
                            millis_for_log(decode_duration),
                            millis_for_log(play_duration),
                            voice_stats.render_fps()
                        );
                    }
                    voice_stats.record_decoded_frame();
                    voice_decoded_frames += 1;
                } else {
                    voice_stats.record_dropped_frame();
                }
                voice_reassembled_frames += 1;
                if args.stats_interval_frames > 0
                    && voice_reassembled_frames.is_multiple_of(args.stats_interval_frames)
                {
                    send_viewer_stats(control, room_id, voice_stream_id, voice_stats).await?;
                }
            }
        }
    }

    if screen_received_packets > 0 {
        send_viewer_stats(control, room_id, screen_stream_id, screen_stats).await?;
    }
    if voice_received_packets > 0 {
        send_viewer_stats(control, room_id, voice_stream_id, voice_stats).await?;
    }

    println!(
        "media-summary role=viewer kind=screen stream_id={} frames={} decoded={} rendered={} packets={} lost={} dropped={} latency_ms={} calibrated_latency_ms={} sender_encode_ms_p50={} sender_encode_ms_p95={} sender_send_ms_p50={} sender_send_ms_p95={} server_queue_ms_p50={} server_queue_ms_p95={} reassembly_ms_p50={} reassembly_ms_p95={} decode_ms_p50={} decode_ms_p95={} render_ms_p50={} render_ms_p95={} render_fps={}",
        screen_stream_id,
        screen_reassembled_frames,
        screen_decoded_frames,
        screen_playback.rendered_frames(),
        screen_received_packets,
        screen_stats.lost_packets,
        screen_stats.dropped_frames,
        screen_stats.estimated_latency_ms,
        screen_stats.calibrated_latency_ms,
        screen_stats.sender_encode_ms_p50(),
        screen_stats.sender_encode_ms_p95(),
        screen_stats.sender_send_ms_p50(),
        screen_stats.sender_send_ms_p95(),
        screen_stats.server_queue_ms_p50(),
        screen_stats.server_queue_ms_p95(),
        screen_stats
            .to_viewer_report(room_id, screen_stream_id)
            .reassembly_ms_p50,
        screen_stats
            .to_viewer_report(room_id, screen_stream_id)
            .reassembly_ms_p95,
        screen_stats
            .to_viewer_report(room_id, screen_stream_id)
            .decode_ms_p50,
        screen_stats
            .to_viewer_report(room_id, screen_stream_id)
            .decode_ms_p95,
        screen_stats
            .to_viewer_report(room_id, screen_stream_id)
            .render_ms_p50,
        screen_stats
            .to_viewer_report(room_id, screen_stream_id)
            .render_ms_p95,
        screen_stats.render_fps()
    );
    println!(
        "media-summary role=viewer kind=voice stream_id={} frames={} decoded={} played={} packets={} lost={} dropped={} latency_ms={} calibrated_latency_ms={} sender_encode_ms_p50={} sender_encode_ms_p95={} sender_send_ms_p50={} sender_send_ms_p95={} server_queue_ms_p50={} server_queue_ms_p95={} reassembly_ms_p50={} reassembly_ms_p95={} decode_ms_p50={} decode_ms_p95={} play_ms_p50={} play_ms_p95={} play_fps={}",
        voice_stream_id,
        voice_reassembled_frames,
        voice_decoded_frames,
        voice_playback
            .as_ref()
            .map(|playback| playback.played_frames())
            .unwrap_or_default(),
        voice_received_packets,
        voice_stats.lost_packets,
        voice_stats.dropped_frames,
        voice_stats.estimated_latency_ms,
        voice_stats.calibrated_latency_ms,
        voice_stats.sender_encode_ms_p50(),
        voice_stats.sender_encode_ms_p95(),
        voice_stats.sender_send_ms_p50(),
        voice_stats.sender_send_ms_p95(),
        voice_stats.server_queue_ms_p50(),
        voice_stats.server_queue_ms_p95(),
        voice_stats
            .to_viewer_report(room_id, voice_stream_id)
            .reassembly_ms_p50,
        voice_stats
            .to_viewer_report(room_id, voice_stream_id)
            .reassembly_ms_p95,
        voice_stats
            .to_viewer_report(room_id, voice_stream_id)
            .decode_ms_p50,
        voice_stats
            .to_viewer_report(room_id, voice_stream_id)
            .decode_ms_p95,
        voice_stats
            .to_viewer_report(room_id, voice_stream_id)
            .render_ms_p50,
        voice_stats
            .to_viewer_report(room_id, voice_stream_id)
            .render_ms_p95,
        voice_stats.render_fps()
    );
    Ok(())
}

async fn run_synthetic_screen_viewer_media(
    control: &crate::transport::quic::ControlClient,
    args: &Args,
    room_id: RoomId,
    stream_id: StreamId,
    clock_sync: ClockSyncEstimate,
) -> anyhow::Result<()> {
    let target_frames = args.synthetic_media_frames()?;
    let frame_interval = args.media_frame_interval()?;
    let frame_interval_ms = frame_interval.as_millis().min(u16::MAX as u128) as u16;
    let mut clock_sync = ClockSyncTracker::new(clock_sync, args.clock_sync_refresh_interval());
    let mut buffer = args.frame_reassembly_buffer(frame_interval_ms);
    let mut decoder = args.video_decoder()?;
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
        let sync_estimate = clock_sync.refresh_if_due(control, "screen-viewer").await?;
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
            stats.record_calibrated_latency(
                frame.sender_capture_time_micros,
                frame.sender_clock_offset_micros,
                packet_receive_time_micros,
                sync_estimate.clock_offset_micros,
            );
            stats.record_sender_timestamps(
                frame.sender_capture_time_micros,
                frame.sender_encode_done_time_micros,
                frame.sender_send_time_micros,
            );
            stats.record_server_queue_latency(
                frame.server_receive_time_micros,
                frame.server_send_time_micros,
            );
            println!(
                "media-recv frame_id={} bytes={} keyframe={} latency_ms={} calibrated_latency_ms={} sender_encode_ms={} sender_send_ms={} server_queue_ms={} reassembly_ms={}",
                frame.frame_id,
                frame.bytes.len(),
                frame.is_keyframe,
                stats.estimated_latency_ms,
                stats.calibrated_latency_ms,
                stats.sender_encode_ms,
                stats.sender_send_ms,
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
        "media-summary role=viewer frames={} decoded={} rendered={} packets={} lost={} dropped={} latency_ms={} calibrated_latency_ms={} sender_encode_ms_p50={} sender_encode_ms_p95={} sender_send_ms_p50={} sender_send_ms_p95={} server_queue_ms_p50={} server_queue_ms_p95={} reassembly_ms_p50={} reassembly_ms_p95={} decode_ms_p50={} decode_ms_p95={} render_ms_p50={} render_ms_p95={} render_fps={}",
        reassembled_frames,
        decoded_frames,
        playback.rendered_frames(),
        received_packets,
        stats.lost_packets,
        stats.dropped_frames,
        stats.estimated_latency_ms,
        stats.calibrated_latency_ms,
        stats.sender_encode_ms_p50(),
        stats.sender_encode_ms_p95(),
        stats.sender_send_ms_p50(),
        stats.sender_send_ms_p95(),
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
    control: &crate::transport::quic::ControlClient,
    args: &Args,
    room_id: RoomId,
    stream_id: StreamId,
    clock_sync: ClockSyncEstimate,
) -> anyhow::Result<()> {
    if args.deafened {
        println!(
            "voice-deafened role=viewer room_id={} stream_id={}",
            room_id, stream_id
        );
        println!(
            "media-summary role=viewer kind=voice deafened=true frames=0 decoded=0 played=0 packets=0 lost=0 dropped=0 latency_ms=0 calibrated_latency_ms=0 sender_encode_ms_p50=0 sender_encode_ms_p95=0 sender_send_ms_p50=0 sender_send_ms_p95=0 server_queue_ms_p50=0 server_queue_ms_p95=0 reassembly_ms_p50=0 reassembly_ms_p95=0 decode_ms_p50=0 decode_ms_p95=0 play_ms_p50=0 play_ms_p95=0 play_fps=0"
        );
        return Ok(());
    }
    let target_frames = args.synthetic_media_frames()?;
    let frame_interval = args.media_frame_interval()?;
    let frame_interval_ms = frame_interval.as_millis().min(u16::MAX as u128) as u16;
    let mut clock_sync = ClockSyncTracker::new(clock_sync, args.clock_sync_refresh_interval());
    let mut buffer = args.frame_reassembly_buffer(frame_interval_ms);
    let mut decoder = SyntheticOpusDecoder;
    let mut playback = args.audio_playback()?;
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
        let sync_estimate = clock_sync.refresh_if_due(control, "voice-viewer").await?;
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
            stats.record_calibrated_latency(
                frame.sender_capture_time_micros,
                frame.sender_clock_offset_micros,
                packet_receive_time_micros,
                sync_estimate.clock_offset_micros,
            );
            stats.record_sender_timestamps(
                frame.sender_capture_time_micros,
                frame.sender_encode_done_time_micros,
                frame.sender_send_time_micros,
            );
            stats.record_server_queue_latency(
                frame.server_receive_time_micros,
                frame.server_send_time_micros,
            );
            println!(
                "audio-recv frame_id={} bytes={} latency_ms={} calibrated_latency_ms={} sender_encode_ms={} sender_send_ms={} server_queue_ms={} reassembly_ms={}",
                frame.frame_id,
                frame.bytes.len(),
                stats.estimated_latency_ms,
                stats.calibrated_latency_ms,
                stats.sender_encode_ms,
                stats.sender_send_ms,
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
        "media-summary role=viewer kind=voice frames={} decoded={} played={} packets={} lost={} dropped={} latency_ms={} calibrated_latency_ms={} sender_encode_ms_p50={} sender_encode_ms_p95={} sender_send_ms_p50={} sender_send_ms_p95={} server_queue_ms_p50={} server_queue_ms_p95={} reassembly_ms_p50={} reassembly_ms_p95={} decode_ms_p50={} decode_ms_p95={} play_ms_p50={} play_ms_p95={} play_fps={}",
        reassembled_frames,
        decoded_frames,
        playback.played_frames(),
        received_packets,
        stats.lost_packets,
        stats.dropped_frames,
        stats.estimated_latency_ms,
        stats.calibrated_latency_ms,
        stats.sender_encode_ms_p50(),
        stats.sender_encode_ms_p95(),
        stats.sender_send_ms_p50(),
        stats.sender_send_ms_p95(),
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
    control: &crate::transport::quic::ControlClient,
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
    control: &crate::transport::quic::ControlClient,
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
    control: &crate::transport::quic::ControlClient,
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

async fn send_keepalive(control: &crate::transport::quic::ControlClient) -> anyhow::Result<()> {
    let nonce = unix_time_micros();
    let response = control.send(ClientControl::Ping(Ping { nonce })).await?;
    match response.message {
        ServerControl::Pong(pong) if pong.nonce == nonce => Ok(()),
        ServerControl::Error(error) => bail!("keepalive failed: {}", error.message),
        other => bail!("unexpected keepalive response: {other:?}"),
    }
}

async fn send_viewer_stats(
    control: &crate::transport::quic::ControlClient,
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

fn print_audio_sources() -> anyhow::Result<()> {
    let sources = audio_capture::list_microphone_sources()?;
    println!("audio-sources count={}", sources.len());
    for source in sources {
        println!("{}", format_audio_source(&source));
    }
    Ok(())
}

fn print_codec_backends() {
    print_h264_encoder_backend(H264VideoEncoderBackend::Synthetic, true);
    print_h264_encoder_backend(H264VideoEncoderBackend::MediaFoundation, false);
    print_h264_decoder_backend(H264VideoDecoderBackend::Synthetic, true);
    print_h264_decoder_backend(H264VideoDecoderBackend::MediaFoundation, false);
    println!(
        "codec-backend kind=audio codec=Opus backend=synthetic role=encoder-decoder available=true hardware=false default=true detail={:?}",
        "synthetic Opus-like test encoder"
    );
}

fn print_h264_encoder_backend(backend: H264VideoEncoderBackend, is_default: bool) {
    let status = h264_encoder_backend_status(backend);
    println!(
        "codec-backend kind=video codec=H264 backend={} role=encoder available={} hardware={} default={} detail={:?}",
        status.backend, status.available, status.hardware, is_default, status.detail
    );
}

fn print_h264_decoder_backend(backend: H264VideoDecoderBackend, is_default: bool) {
    let status = h264_decoder_backend_status(backend);
    println!(
        "codec-backend kind=video codec=H264 backend={} role=decoder available={} hardware={} default={} detail={:?}",
        status.backend, status.available, status.hardware, is_default, status.detail
    );
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

fn format_audio_source(source: &MicrophoneSourceInfo) -> String {
    match &source.source {
        MicrophoneSource::Default => format!(
            "audio-source kind=default default={} sample_rate_hz={} channels={} label={:?}",
            source.is_default, source.sample_rate_hz, source.channel_count, source.label
        ),
        MicrophoneSource::Device { id } => format!(
            "audio-source kind=device id={} default={} sample_rate_hz={} channels={} label={:?}",
            id, source.is_default, source.sample_rate_hz, source.channel_count, source.label
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
    fn control_display_name(&self) -> String {
        self.display_name
            .clone()
            .unwrap_or_else(|| format!("desktop-client/{:?}", self.mode))
    }

    fn selected_room_id(&self) -> anyhow::Result<Option<RoomId>> {
        match (self.room_id, self.channel_id) {
            (Some(room_id), Some(channel_id)) if room_id != channel_id => {
                bail!("--room-id and --channel-id must match when both are set")
            }
            (Some(room_id), _) => Ok(Some(room_id)),
            (_, Some(channel_id)) => Ok(Some(channel_id)),
            (None, None) => Ok(None),
        }
    }

    fn selected_channel_name(&self) -> anyhow::Result<&str> {
        match self.channel_name.as_deref() {
            Some(channel_name)
                if self.room_name != DEFAULT_CHANNEL_NAME && self.room_name != channel_name =>
            {
                bail!("--room-name and --channel-name must match when both are set")
            }
            Some(channel_name) => Ok(channel_name),
            None => Ok(&self.room_name),
        }
    }

    fn synthetic_media_enabled(&self) -> bool {
        self.media_frames > 0 || self.media_run_ms > 0
    }

    fn voice_state_requested(&self) -> bool {
        self.muted || self.deafened || self.voice_push_to_talk_enabled()
    }

    fn voice_push_to_talk_enabled(&self) -> bool {
        self.push_to_talk || self.ptt_active
    }

    fn voice_speaking(&self) -> bool {
        !self.muted && (!self.voice_push_to_talk_enabled() || self.ptt_active)
    }

    fn synthetic_media_frames(&self) -> anyhow::Result<u32> {
        self.synthetic_media_frames_for_fps(self.active_media_fps(), "--media-fps/--voice-fps")
    }

    fn synthetic_media_frames_for_fps(
        &self,
        frames_per_second: u16,
        fps_flag: &str,
    ) -> anyhow::Result<u32> {
        if self.media_frames > 0 {
            return Ok(self.media_frames);
        }
        if frames_per_second == 0 {
            bail!("{fps_flag} must be greater than zero");
        }
        let frames = self
            .media_run_ms
            .saturating_mul(frames_per_second as u64)
            .div_ceil(1_000)
            .max(1);
        Ok(frames.min(u32::MAX as u64) as u32)
    }

    fn media_frame_interval(&self) -> anyhow::Result<Duration> {
        self.media_frame_interval_for_fps(self.active_media_fps(), "--media-fps/--voice-fps")
    }

    fn media_frame_interval_for_fps(
        &self,
        frames_per_second: u16,
        fps_flag: &str,
    ) -> anyhow::Result<Duration> {
        if frames_per_second == 0 {
            bail!("{fps_flag} must be greater than zero");
        }
        Ok(Duration::from_micros(media_interval_micros(
            frames_per_second,
        )))
    }

    fn active_media_fps(&self) -> u16 {
        match self.media_kind {
            MediaKindArg::Voice => self.voice_fps,
            MediaKindArg::Screen | MediaKindArg::Both => self.media_fps,
        }
    }

    fn voice_frames_per_second(&self) -> u16 {
        self.voice_fps
    }

    fn clock_sync_refresh_interval(&self) -> Option<Duration> {
        (self.time_sync_refresh_ms > 0).then(|| Duration::from_millis(self.time_sync_refresh_ms))
    }

    fn frame_reassembly_buffer(&self, frame_interval_ms: u16) -> FrameReassemblyBuffer {
        FrameReassemblyBuffer::with_jitter_budget(
            64,
            self.reassembly_window_frames,
            self.jitter_buffer_max_ms,
            frame_interval_ms.max(1),
        )
    }

    fn synthetic_bitrate_bps(&self) -> u32 {
        let bitrate = (self.active_media_frame_bytes() as u128)
            .saturating_mul(self.active_media_fps().max(1) as u128)
            .saturating_mul(8);
        bitrate.min(u32::MAX as u128) as u32
    }

    fn active_media_frame_bytes(&self) -> usize {
        match self.media_kind {
            MediaKindArg::Voice => self.voice_frame_bytes,
            MediaKindArg::Screen | MediaKindArg::Both => self.media_frame_bytes,
        }
    }

    fn video_encoder_backend(&self) -> H264VideoEncoderBackend {
        match self.video_encoder {
            VideoEncoderArg::Synthetic => H264VideoEncoderBackend::Synthetic,
            VideoEncoderArg::MediaFoundation => H264VideoEncoderBackend::MediaFoundation,
        }
    }

    fn video_encoder(&self, width: u32, height: u32) -> anyhow::Result<H264VideoEncoder> {
        H264VideoEncoder::new(
            self.video_encoder_backend(),
            H264EncoderConfig {
                width,
                height,
                frames_per_second: self.media_fps.max(1),
                bitrate_bps: self.synthetic_bitrate_bps(),
                synthetic_payload_bytes: self.media_frame_bytes,
            },
        )
    }

    fn video_decoder_backend(&self) -> H264VideoDecoderBackend {
        match self.video_decoder {
            VideoDecoderArg::Synthetic => H264VideoDecoderBackend::Synthetic,
            VideoDecoderArg::MediaFoundation => H264VideoDecoderBackend::MediaFoundation,
        }
    }

    fn video_decoder(&self) -> anyhow::Result<H264VideoDecoder> {
        H264VideoDecoder::new(self.video_decoder_backend())
    }

    fn protocol_media_kind(&self) -> anyhow::Result<MediaKind> {
        match self.media_kind {
            MediaKindArg::Screen => Ok(MediaKind::Screen),
            MediaKindArg::Voice => Ok(MediaKind::Voice),
            MediaKindArg::Both => {
                bail!("--media-kind both does not map to a single protocol media kind")
            }
        }
    }

    fn codec(&self) -> anyhow::Result<CodecId> {
        match self.media_kind {
            MediaKindArg::Screen => Ok(CodecId::H264),
            MediaKindArg::Voice => Ok(CodecId::Opus),
            MediaKindArg::Both => bail!("--media-kind both does not map to a single codec"),
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
                frames_per_second: self.voice_frames_per_second().max(1),
                timebase_hz: 48_000,
            }),
            MediaKindArg::Both => bail!("--media-kind both does not map to a single stream config"),
        }
    }

    fn voice_stream_id(&self) -> anyhow::Result<StreamId> {
        let voice_stream_id = self
            .voice_stream_id
            .unwrap_or_else(|| self.stream_id.saturating_add(1));
        if voice_stream_id == self.stream_id {
            bail!("--voice-stream-id must be different from --stream-id for --media-kind both");
        }
        Ok(voice_stream_id)
    }

    fn for_media_kind(&self, media_kind: MediaKindArg, stream_id: StreamId) -> Self {
        let mut args = self.clone();
        args.media_kind = media_kind;
        args.stream_id = stream_id;
        args
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

    fn microphone_source(&self) -> anyhow::Result<MicrophoneSource> {
        match self.microphone_id.as_deref().map(str::trim) {
            Some(id) if !id.is_empty() => Ok(MicrophoneSource::Device { id: id.to_owned() }),
            _ => Ok(MicrophoneSource::Default),
        }
    }

    fn audio_capture_config(&self) -> AudioCaptureConfig {
        AudioCaptureConfig {
            sample_rate_hz: 48_000,
            channel_count: 1,
            frame_duration_ms: audio_frame_duration_ms(self.voice_frames_per_second()),
            queue_capacity: 1,
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

    fn audio_playback(&self) -> anyhow::Result<AudioOutputPlayback> {
        match self.audio_output {
            AudioOutputArg::Sink => Ok(AudioOutputPlayback::sink()),
            AudioOutputArg::Speaker => {
                AudioOutputPlayback::speaker().context("failed to create speaker audio output")
            }
        }
    }

    fn remote_input_applier(&self) -> anyhow::Result<Box<dyn RemoteInputApplier>> {
        match self.remote_input_output {
            RemoteInputOutputArg::Log => Ok(Box::new(LoggingRemoteInputApplier::default())),
            RemoteInputOutputArg::Native => Ok(Box::new(
                NativeRemoteInputApplier::new().context("failed to create native input applier")?,
            )),
        }
    }

    fn remote_input_script_events(&self, script: RemoteInputScriptArg) -> Vec<RemoteInputKind> {
        match script {
            RemoteInputScriptArg::PointerTap => vec![
                RemoteInputKind::PointerButton {
                    button: PointerButton::Left,
                    pressed: true,
                    normalized_x: 32_768,
                    normalized_y: 32_768,
                },
                RemoteInputKind::PointerButton {
                    button: PointerButton::Left,
                    pressed: false,
                    normalized_x: 32_768,
                    normalized_y: 32_768,
                },
            ],
            RemoteInputScriptArg::KeyEnter => vec![
                RemoteInputKind::Key {
                    key_code: 13,
                    pressed: true,
                },
                RemoteInputKind::Key {
                    key_code: 13,
                    pressed: false,
                },
            ],
            RemoteInputScriptArg::Text => vec![RemoteInputKind::Text {
                text: self
                    .remote_input_text
                    .clone()
                    .unwrap_or_else(|| "hello from viewer".to_owned()),
            }],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stream_summary(stream_id: StreamId, media_kind: MediaKind) -> StreamSummary {
        StreamSummary {
            room_id: 1,
            stream_id,
            publisher_id: 10,
            codec: match media_kind {
                MediaKind::Screen => CodecId::H264,
                MediaKind::Voice => CodecId::Opus,
                MediaKind::Probe => CodecId::H264,
            },
            media_kind,
            subscriber_count: 0,
            has_config: true,
            target_bitrate_bps: 800_000,
            target_frames_per_second: 30,
        }
    }

    fn participant_summary(user_id: u64) -> ParticipantSummary {
        ParticipantSummary {
            room_id: 1,
            user_id,
            display_name: format!("user-{user_id}"),
            muted: false,
            deafened: true,
            push_to_talk: true,
            speaking: false,
            published_stream_count: 1,
            subscribed_stream_count: 2,
        }
    }

    #[test]
    fn list_capture_sources_flag_parses_without_relay_options() {
        let args = Args::try_parse_from(["desktop-client", "--list-capture-sources"]).unwrap();

        assert!(args.list_capture_sources);
    }

    #[test]
    fn list_audio_sources_flag_parses_without_relay_options() {
        let args = Args::try_parse_from(["desktop-client", "--list-audio-sources"]).unwrap();

        assert!(args.list_audio_sources);
    }

    #[test]
    fn list_codec_backends_flag_parses_without_relay_options() {
        let args = Args::try_parse_from(["desktop-client", "--list-codec-backends"]).unwrap();

        assert!(args.list_codec_backends);
    }

    #[test]
    fn list_rooms_flag_parses_without_media_options() {
        let args = Args::try_parse_from(["desktop-client", "--list-rooms"]).unwrap();

        assert!(args.list_rooms);
    }

    #[test]
    fn list_streams_flag_parses_without_media_options() {
        let args = Args::try_parse_from(["desktop-client", "--list-streams"]).unwrap();

        assert!(args.list_streams);
    }

    #[test]
    fn list_participants_flag_parses_without_media_options() {
        let args = Args::try_parse_from(["desktop-client", "--list-participants"]).unwrap();

        assert!(args.list_participants);
    }

    #[test]
    fn display_name_overrides_default_control_name() {
        let named = Args::try_parse_from(["desktop-client", "--display-name", "Alice Screenshare"])
            .unwrap();
        let default = Args::try_parse_from(["desktop-client"]).unwrap();

        assert_eq!(named.control_display_name(), "Alice Screenshare");
        assert_eq!(default.control_display_name(), "desktop-client/Viewer");
    }

    #[test]
    fn channel_aliases_select_room_options() {
        let args = Args::try_parse_from([
            "desktop-client",
            "--channel-id",
            "42",
            "--channel-name",
            "ops-live",
        ])
        .unwrap();

        assert_eq!(args.selected_room_id().unwrap(), Some(42));
        assert_eq!(args.selected_channel_name().unwrap(), "ops-live");
    }

    #[test]
    fn room_and_channel_ids_must_match() {
        let args =
            Args::try_parse_from(["desktop-client", "--room-id", "41", "--channel-id", "42"])
                .unwrap();

        let error = args.selected_room_id().unwrap_err();

        assert!(error.to_string().contains("--channel-id"));
    }

    #[test]
    fn room_and_channel_names_must_match_when_room_name_is_overridden() {
        let args = Args::try_parse_from([
            "desktop-client",
            "--room-name",
            "stage1-old",
            "--channel-name",
            "stage1",
        ])
        .unwrap();

        let error = args.selected_channel_name().unwrap_err();

        assert!(error.to_string().contains("--channel-name"));
    }

    #[test]
    fn voice_input_microphone_flag_selects_microphone_source() {
        let args = Args::try_parse_from([
            "desktop-client",
            "--media-kind",
            "voice",
            "--voice-input",
            "microphone",
            "--microphone-id",
            "1",
        ])
        .unwrap();

        assert_eq!(args.voice_input, VoiceInputArg::Microphone);
        assert_eq!(
            args.microphone_source().unwrap(),
            MicrophoneSource::Device { id: "1".to_owned() }
        );
        assert_eq!(args.audio_capture_config().frame_duration_ms, 20);
    }

    #[test]
    fn screen_media_defaults_to_thirty_fps() {
        let args = Args::try_parse_from([
            "desktop-client",
            "--media-kind",
            "screen",
            "--media-run-ms",
            "1000",
        ])
        .unwrap();

        assert_eq!(args.active_media_fps(), DEFAULT_SCREEN_FPS);
        assert_eq!(args.active_media_frame_bytes(), 512);
        assert_eq!(args.synthetic_media_frames().unwrap(), 30);
        assert_eq!(
            args.media_frame_interval().unwrap(),
            Duration::from_micros(33_333)
        );
        assert_eq!(
            args.stream_config(1).unwrap().frames_per_second,
            DEFAULT_SCREEN_FPS
        );
    }

    #[test]
    fn voice_media_defaults_to_twenty_millisecond_frames() {
        let args = Args::try_parse_from([
            "desktop-client",
            "--media-kind",
            "voice",
            "--media-run-ms",
            "1000",
        ])
        .unwrap();

        assert_eq!(args.active_media_fps(), DEFAULT_VOICE_FPS);
        assert_eq!(args.active_media_frame_bytes(), DEFAULT_VOICE_FRAME_BYTES);
        assert_eq!(args.synthetic_media_frames().unwrap(), 50);
        assert_eq!(
            args.media_frame_interval().unwrap(),
            Duration::from_millis(20)
        );
        assert_eq!(
            args.stream_config(1).unwrap().frames_per_second,
            DEFAULT_VOICE_FPS
        );
        assert_eq!(args.audio_capture_config().frame_duration_ms, 20);
        assert_eq!(
            args.synthetic_bitrate_bps(),
            (DEFAULT_VOICE_FRAME_BYTES * DEFAULT_VOICE_FPS as usize * 8) as u32
        );
    }

    #[test]
    fn voice_fps_overrides_audio_cadence() {
        let args = Args::try_parse_from([
            "desktop-client",
            "--media-kind",
            "voice",
            "--media-run-ms",
            "1000",
            "--voice-fps",
            "40",
        ])
        .unwrap();

        assert_eq!(args.active_media_fps(), 40);
        assert_eq!(args.synthetic_media_frames().unwrap(), 40);
        assert_eq!(
            args.media_frame_interval().unwrap(),
            Duration::from_millis(25)
        );
        assert_eq!(args.audio_capture_config().frame_duration_ms, 25);
    }

    #[test]
    fn voice_frame_bytes_override_audio_payload() {
        let args = Args::try_parse_from([
            "desktop-client",
            "--media-kind",
            "voice",
            "--voice-fps",
            "40",
            "--voice-frame-bytes",
            "120",
        ])
        .unwrap();

        assert_eq!(args.active_media_frame_bytes(), 120);
        assert_eq!(args.synthetic_bitrate_bps(), 38_400);
    }

    #[test]
    fn voice_input_uses_default_microphone_without_device_id() {
        let args = Args::try_parse_from([
            "desktop-client",
            "--media-kind",
            "voice",
            "--voice-input",
            "microphone",
        ])
        .unwrap();

        assert_eq!(args.microphone_source().unwrap(), MicrophoneSource::Default);
    }

    #[test]
    fn audio_output_flag_selects_speaker() {
        let args = Args::try_parse_from([
            "desktop-client",
            "--media-kind",
            "voice",
            "--audio-output",
            "speaker",
        ])
        .unwrap();

        assert_eq!(args.audio_output, AudioOutputArg::Speaker);
    }

    #[test]
    fn video_encoder_flag_selects_media_foundation_backend() {
        let args = Args::try_parse_from(["desktop-client", "--video-encoder", "media-foundation"])
            .unwrap();

        assert_eq!(args.video_encoder, VideoEncoderArg::MediaFoundation);
        assert_eq!(
            args.video_encoder_backend(),
            H264VideoEncoderBackend::MediaFoundation
        );
    }

    #[test]
    fn video_decoder_flag_selects_media_foundation_backend() {
        let args = Args::try_parse_from(["desktop-client", "--video-decoder", "media-foundation"])
            .unwrap();

        assert_eq!(args.video_decoder, VideoDecoderArg::MediaFoundation);
        assert_eq!(
            args.video_decoder_backend(),
            H264VideoDecoderBackend::MediaFoundation
        );
    }

    #[test]
    fn voice_state_flags_parse() {
        let args = Args::try_parse_from(["desktop-client", "--muted", "--deafened"]).unwrap();

        assert!(args.muted);
        assert!(args.deafened);
        assert!(!args.voice_speaking());
    }

    #[test]
    fn push_to_talk_requires_active_press_to_speak() {
        let idle =
            Args::try_parse_from(["desktop-client", "--media-kind", "voice", "--push-to-talk"])
                .unwrap();
        let active = Args::try_parse_from([
            "desktop-client",
            "--media-kind",
            "voice",
            "--push-to-talk",
            "--ptt-active",
        ])
        .unwrap();

        assert!(idle.voice_push_to_talk_enabled());
        assert!(!idle.voice_speaking());
        assert!(active.voice_push_to_talk_enabled());
        assert!(active.voice_speaking());
    }

    #[test]
    fn media_kind_both_derives_voice_stream_id() {
        let args = Args::try_parse_from([
            "desktop-client",
            "--mode",
            "broadcaster",
            "--media-kind",
            "both",
            "--stream-id",
            "7",
        ])
        .unwrap();

        assert_eq!(args.media_kind, MediaKindArg::Both);
        assert_eq!(args.voice_stream_id().unwrap(), 8);

        let voice_args = args.for_media_kind(MediaKindArg::Voice, args.voice_stream_id().unwrap());
        assert_eq!(voice_args.media_kind, MediaKindArg::Voice);
        assert_eq!(voice_args.stream_id, 8);
        assert_eq!(voice_args.codec().unwrap(), CodecId::Opus);
        assert_eq!(voice_args.active_media_fps(), DEFAULT_VOICE_FPS);
        assert_eq!(
            voice_args.active_media_frame_bytes(),
            DEFAULT_VOICE_FRAME_BYTES
        );
        assert_eq!(
            voice_args.stream_config(1).unwrap().frames_per_second,
            DEFAULT_VOICE_FPS
        );
    }

    #[test]
    fn dual_stream_args_keep_screen_and_voice_payloads_independent() {
        let args = Args::try_parse_from([
            "desktop-client",
            "--media-kind",
            "both",
            "--media-frame-bytes",
            "800",
            "--voice-frame-bytes",
            "96",
            "--stream-id",
            "1",
            "--voice-stream-id",
            "2",
        ])
        .unwrap();

        let screen_args = args.for_media_kind(MediaKindArg::Screen, args.stream_id);
        let voice_args = args.for_media_kind(MediaKindArg::Voice, args.voice_stream_id().unwrap());

        assert_eq!(screen_args.active_media_frame_bytes(), 800);
        assert_eq!(voice_args.active_media_frame_bytes(), 96);
        assert_eq!(screen_args.synthetic_bitrate_bps(), 192_000);
        assert_eq!(voice_args.synthetic_bitrate_bps(), 38_400);
    }

    #[test]
    fn media_kind_both_rejects_conflicting_voice_stream_id() {
        let args = Args::try_parse_from([
            "desktop-client",
            "--media-kind",
            "both",
            "--stream-id",
            "3",
            "--voice-stream-id",
            "3",
        ])
        .unwrap();

        let error = args.voice_stream_id().unwrap_err();

        assert!(error.to_string().contains("--voice-stream-id"));
    }

    #[test]
    fn media_kind_both_has_no_single_stream_config() {
        let args = Args::try_parse_from(["desktop-client", "--media-kind", "both"]).unwrap();

        assert!(args.stream_config(1).is_err());
        assert!(args.protocol_media_kind().is_err());
    }

    #[test]
    fn dual_viewer_resolves_screen_and_voice_streams() {
        let args = Args::try_parse_from([
            "desktop-client",
            "--mode",
            "viewer",
            "--media-kind",
            "both",
            "--stream-id",
            "1",
            "--voice-stream-id",
            "2",
        ])
        .unwrap();
        let streams = [
            stream_summary(1, MediaKind::Screen),
            stream_summary(2, MediaKind::Voice),
        ];

        let resolved = resolve_dual_viewer_stream_ids(&args, &streams).unwrap();

        assert_eq!(resolved, (1, 2));
    }

    #[test]
    fn dual_viewer_rejects_wrong_voice_stream_kind() {
        let args = Args::try_parse_from([
            "desktop-client",
            "--mode",
            "viewer",
            "--media-kind",
            "both",
            "--stream-id",
            "1",
            "--voice-stream-id",
            "2",
        ])
        .unwrap();
        let streams = [
            stream_summary(1, MediaKind::Screen),
            stream_summary(2, MediaKind::Screen),
        ];

        let error = resolve_dual_viewer_stream_ids(&args, &streams).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("stream 2 is Screen, but viewer requested Voice")
        );
    }

    #[test]
    fn clock_sync_estimate_uses_midpoints_and_excludes_server_processing() {
        let sync = TimeSyncResponse {
            client_send_time_micros: 1_000,
            server_receive_time_micros: 1_800,
            server_send_time_micros: 2_000,
        };

        let estimate = estimate_clock_sync(&sync, 2_400);

        assert_eq!(
            estimate,
            ClockSyncEstimate {
                rtt_micros: 1_200,
                rtt_ms: 1,
                clock_offset_micros: 200,
            }
        );
    }

    #[test]
    fn clock_sync_estimate_saturates_rtt_millis() {
        let sync = TimeSyncResponse {
            client_send_time_micros: 0,
            server_receive_time_micros: 0,
            server_send_time_micros: 0,
        };

        let estimate = estimate_clock_sync(&sync, u64::MAX);

        assert_eq!(estimate.rtt_ms, u16::MAX);
        assert_eq!(estimate.rtt_micros, u64::MAX);
    }

    #[test]
    fn clock_sync_selection_prefers_lowest_rtt_sample() {
        let samples = [
            (
                1,
                ClockSyncEstimate {
                    rtt_micros: 2_000,
                    rtt_ms: 2,
                    clock_offset_micros: 50,
                },
            ),
            (
                2,
                ClockSyncEstimate {
                    rtt_micros: 900,
                    rtt_ms: 0,
                    clock_offset_micros: 75,
                },
            ),
            (
                3,
                ClockSyncEstimate {
                    rtt_micros: 1_400,
                    rtt_ms: 1,
                    clock_offset_micros: 60,
                },
            ),
        ];

        assert_eq!(
            select_best_clock_sync_estimate(&samples),
            Some((2, samples[1].1))
        );
    }

    #[test]
    fn clock_sync_refresh_interval_can_be_disabled() {
        let default_args = Args::try_parse_from(["desktop-client"]).unwrap();
        let disabled_args =
            Args::try_parse_from(["desktop-client", "--time-sync-refresh-ms", "0"]).unwrap();

        assert_eq!(
            default_args.clock_sync_refresh_interval(),
            Some(Duration::from_millis(5_000))
        );
        assert_eq!(disabled_args.clock_sync_refresh_interval(), None);
    }

    #[test]
    fn jitter_buffer_max_ms_defaults_to_low_latency_budget() {
        let default_args = Args::try_parse_from(["desktop-client"]).unwrap();
        let custom_args =
            Args::try_parse_from(["desktop-client", "--jitter-buffer-max-ms", "80"]).unwrap();

        assert_eq!(default_args.jitter_buffer_max_ms, 150);
        assert_eq!(custom_args.jitter_buffer_max_ms, 80);
    }

    #[test]
    fn remote_input_script_flag_builds_events() {
        let tap = Args::try_parse_from(["desktop-client", "--remote-input-script", "pointer-tap"])
            .unwrap();
        let text = Args::try_parse_from([
            "desktop-client",
            "--remote-input-script",
            "text",
            "--remote-input-text",
            "hi",
        ])
        .unwrap();

        assert_eq!(
            tap.remote_input_script,
            Some(RemoteInputScriptArg::PointerTap)
        );
        assert_eq!(tap.remote_input_output, RemoteInputOutputArg::Log);
        assert_eq!(
            tap.remote_input_script_events(RemoteInputScriptArg::PointerTap)
                .len(),
            2
        );
        assert_eq!(
            text.remote_input_script_events(RemoteInputScriptArg::Text),
            vec![RemoteInputKind::Text {
                text: "hi".to_owned()
            }]
        );
    }

    #[test]
    fn remote_input_output_flag_selects_native_mode() {
        let args =
            Args::try_parse_from(["desktop-client", "--remote-input-output", "native"]).unwrap();

        assert_eq!(args.remote_input_output, RemoteInputOutputArg::Native);
    }

    #[test]
    fn clock_sync_tracker_keeps_current_estimate_when_refresh_disabled() {
        let estimate = ClockSyncEstimate {
            rtt_micros: 1_000,
            rtt_ms: 1,
            clock_offset_micros: 42,
        };
        let tracker = ClockSyncTracker::new(estimate, None);

        assert_eq!(tracker.estimate(), estimate);
    }

    #[tokio::test]
    async fn publisher_feedback_updates_screen_resolution_target() {
        let feedback = PublisherFeedback {
            room_id: 1,
            stream_id: 9,
            aggregate_available_bitrate_bps: 2_000_000,
            target_frames_per_second: 24,
            target_width: 1024,
            target_height: 576,
            degraded_viewer_count: 1,
            total_viewer_count: 1,
            keyframe_requested: true,
        };
        let mut encoder = H264Encoder::default();
        encoder.config.bitrate_bps = 4_000_000;
        encoder.config.synthetic_payload_bytes = 512;
        let mut active_fps = 30;
        let mut target_width = 1280;
        let mut target_height = 720;
        let mut ticker = tokio::time::interval(Duration::from_millis(33));

        let resolution_changed = apply_publisher_feedback(
            &feedback,
            &mut encoder,
            &mut active_fps,
            &mut target_width,
            &mut target_height,
            &mut ticker,
        );

        assert!(resolution_changed);
        assert_eq!(encoder.config.bitrate_bps, 2_000_000);
        assert_eq!(active_fps, 24);
        assert_eq!(target_width, 1024);
        assert_eq!(target_height, 576);
        assert_eq!(encoder.config.width, 1024);
        assert_eq!(encoder.config.height, 576);
        assert!(encoder.keyframe_requested);
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
    fn format_audio_source_prints_device_metadata() {
        let source = MicrophoneSourceInfo {
            source: MicrophoneSource::Device { id: "0".to_owned() },
            label: "Microphone Array".to_owned(),
            sample_rate_hz: 48_000,
            channel_count: 2,
            is_default: true,
        };

        assert_eq!(
            format_audio_source(&source),
            "audio-source kind=device id=0 default=true sample_rate_hz=48000 channels=2 label=\"Microphone Array\""
        );
    }

    #[test]
    fn format_participant_summary_prints_voice_state() {
        let summary = participant_summary(7);

        assert_eq!(
            format_participant_summary(&summary),
            "participant room_id=1 user_id=7 display_name=\"user-7\" muted=false deafened=true push_to_talk=true speaking=false published_streams=1 subscribed_streams=2"
        );
    }

    #[test]
    fn format_room_summary_prints_discovery_line() {
        let room = RoomSummary {
            room_id: 3,
            name: "stage1".to_owned(),
            participant_count: 2,
            published_stream_count: 1,
        };

        assert_eq!(
            format_room_summary(&room),
            "room room_id=3 name=\"stage1\" participants=2 streams=1"
        );
    }

    #[test]
    fn format_stream_summary_prints_discovery_line() {
        let stream = stream_summary(9, MediaKind::Screen);

        assert_eq!(
            format_stream_summary(&stream),
            "stream room_id=1 stream_id=9 publisher_id=10 codec=H264 media_kind=Screen subscribers=0 configured=true target_bitrate_bps=800000 target_fps=30"
        );
    }

    #[test]
    fn format_remote_input_event_prints_event_details() {
        let event = RemoteInputEvent {
            sender_user_id: 7,
            sequence_number: 2,
            event_time_micros: 123,
            kind: RemoteInputKind::Key {
                key_code: 13,
                pressed: false,
            },
        };

        assert_eq!(
            format_remote_input_event(&event),
            "remote-input event=key sender_user_id=7 sequence=2 event_time_micros=123 key_code=13 pressed=false"
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

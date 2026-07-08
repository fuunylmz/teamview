use bytes::Bytes;
use clap::{Parser, ValueEnum};
use relay_server::router::StreamFanout;
use teamview_protocol::{
    codec::CodecId,
    control::UserId,
    frame::{EncodedFrame, packetize_frame, reassemble_frame},
    packet::{
        DEFAULT_DATAGRAM_PAYLOAD_TARGET, MediaPacket, MediaPacketHeader, PacketFlags, PacketType,
    },
};
use tracing::info;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Mode {
    Fanout,
    SampleForward,
}

#[derive(Debug, Parser)]
#[command(author, version, about = "Synthetic TeamView relay load test scaffold")]
struct Args {
    #[arg(long, value_enum, default_value_t = Mode::Fanout)]
    mode: Mode,

    #[arg(long, default_value_t = 1)]
    publishers: u16,

    #[arg(long, default_value_t = 10)]
    viewers: u16,

    #[arg(long, default_value_t = 120)]
    packets: u32,

    #[arg(long, default_value_t = 20)]
    media_duration_ms: u16,

    #[arg(long, default_value_t = DEFAULT_DATAGRAM_PAYLOAD_TARGET)]
    max_payload: usize,

    #[arg(long, default_value_t = false)]
    include_slow_viewer: bool,
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let args = Args::parse();
    match args.mode {
        Mode::Fanout => {
            let result = run_synthetic_fanout(&args);
            info!(?args, ?result, "load-test synthetic fanout complete");
            println!(
                "synthetic-fanout publishers={} viewers={} packets={} delivered={} dropped={} slow_viewer_dropped={}",
                args.publishers,
                args.viewers,
                args.packets,
                result.total_delivered,
                result.total_dropped,
                result.slow_viewer_dropped
            );
        }
        Mode::SampleForward => {
            let result = run_sample_forward(&args)?;
            info!(?args, ?result, "load-test sample forward complete");
            println!(
                "sample-forward frames={} fragments={} reassembled={} delivered={} dropped={}",
                result.frames,
                result.fragments,
                result.reassembled_frames,
                result.total_delivered,
                result.total_dropped
            );
        }
    }

    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct SyntheticFanoutResult {
    total_delivered: u64,
    total_dropped: u64,
    slow_viewer_dropped: u64,
}

fn run_synthetic_fanout(args: &Args) -> SyntheticFanoutResult {
    let mut result = SyntheticFanoutResult::default();
    for publisher_index in 0..args.publishers {
        let stream_id = publisher_index as u32 + 1;
        let mut fanout = StreamFanout::new(stream_id);
        for viewer in 1..=args.viewers as UserId {
            let budget = if args.include_slow_viewer && viewer == args.viewers as UserId {
                args.media_duration_ms
            } else {
                args.media_duration_ms.saturating_mul(10)
            };
            fanout.add_viewer(viewer, budget);
        }

        for packet_index in 0..args.packets {
            for viewer in 1..args.viewers as UserId {
                fanout
                    .viewer_queue_mut(viewer)
                    .and_then(|queue| queue.drain_one());
            }
            let packet = synthetic_packet(stream_id, packet_index + 1, packet_index + 1);
            let summary = fanout.fanout(packet, args.media_duration_ms);
            result.total_delivered += summary.delivered_to.len() as u64;
            result.total_dropped += summary.dropped_for.len() as u64;
            if args.include_slow_viewer && summary.dropped_for.contains(&(args.viewers as UserId)) {
                result.slow_viewer_dropped += 1;
            }
        }
    }
    result
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct SampleForwardResult {
    frames: u32,
    fragments: u64,
    reassembled_frames: u64,
    total_delivered: u64,
    total_dropped: u64,
}

fn run_sample_forward(args: &Args) -> anyhow::Result<SampleForwardResult> {
    let mut result = SampleForwardResult::default();
    let mut fanout = StreamFanout::new(1);
    for viewer in 1..=args.viewers as UserId {
        fanout.add_viewer(viewer, args.media_duration_ms.saturating_mul(10));
    }

    for frame_index in 0..args.packets {
        for viewer in 1..=args.viewers as UserId {
            while fanout
                .viewer_queue_mut(viewer)
                .and_then(|queue| queue.drain_one())
                .is_some()
            {}
        }

        let frame = sample_h264_frame(frame_index + 1);
        let packets = packetize_frame(&frame, frame_index.saturating_mul(100), args.max_payload)?;
        result.frames += 1;
        result.fragments += packets.len() as u64;

        let mut first_viewer_packets = Vec::new();
        for packet in packets {
            let summary = fanout.fanout(packet.clone(), args.media_duration_ms);
            result.total_delivered += summary.delivered_to.len() as u64;
            result.total_dropped += summary.dropped_for.len() as u64;
            if summary.delivered_to.contains(&1) {
                first_viewer_packets.push(packet);
            }
        }

        let reassembled = reassemble_frame(first_viewer_packets)?;
        if reassembled.bytes != frame.bytes {
            anyhow::bail!(
                "reassembled frame bytes differ for frame {}",
                frame.frame_id
            );
        }
        result.reassembled_frames += 1;
    }

    Ok(result)
}

fn sample_h264_frame(frame_id: u32) -> EncodedFrame {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x67, 0x42, 0x00, 0x1f]);
    bytes.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x68, 0xce, 0x06, 0xe2]);
    bytes.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x65]);
    for idx in 0..4096 {
        bytes.push(frame_id.wrapping_add(idx).to_le_bytes()[0]);
    }

    EncodedFrame {
        room_stream_id: 1,
        frame_id,
        media_timestamp: frame_id as u64 * 3_000,
        sender_capture_time_micros: frame_id as u64 * 33_333,
        codec: CodecId::H264,
        is_keyframe: frame_id == 1,
        bytes: Bytes::from(bytes),
    }
}

fn synthetic_packet(stream_id: u32, sequence_number: u32, frame_id: u32) -> MediaPacket {
    let payload = Bytes::from_static(b"synthetic-stage2-payload");
    let mut header = MediaPacketHeader::new(
        PacketType::Video,
        CodecId::H264,
        stream_id,
        sequence_number,
        payload.len() as u16,
    );
    header.frame_id = frame_id;
    header.flags = PacketFlags::empty().with(PacketFlags::END_OF_FRAME);
    MediaPacket { header, payload }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthetic_fanout_without_slow_viewer_has_no_drops() {
        let args = Args {
            mode: Mode::Fanout,
            publishers: 1,
            viewers: 4,
            packets: 10,
            media_duration_ms: 20,
            max_payload: DEFAULT_DATAGRAM_PAYLOAD_TARGET,
            include_slow_viewer: false,
        };

        let result = run_synthetic_fanout(&args);

        assert_eq!(result.total_dropped, 0);
        assert_eq!(result.total_delivered, 40);
    }

    #[test]
    fn synthetic_fanout_isolates_slow_viewer() {
        let args = Args {
            mode: Mode::Fanout,
            publishers: 1,
            viewers: 4,
            packets: 10,
            media_duration_ms: 20,
            max_payload: DEFAULT_DATAGRAM_PAYLOAD_TARGET,
            include_slow_viewer: true,
        };

        let result = run_synthetic_fanout(&args);

        assert_eq!(result.slow_viewer_dropped, 9);
        assert_eq!(result.total_dropped, 9);
        assert_eq!(result.total_delivered, 31);
    }

    #[test]
    fn sample_forward_reassembles_h264_like_frame() {
        let args = Args {
            mode: Mode::SampleForward,
            publishers: 1,
            viewers: 2,
            packets: 3,
            media_duration_ms: 20,
            max_payload: 700,
            include_slow_viewer: false,
        };

        let result = run_sample_forward(&args).unwrap();

        assert_eq!(result.frames, 3);
        assert_eq!(result.reassembled_frames, 3);
        assert!(result.fragments > result.frames as u64);
        assert_eq!(result.total_dropped, 0);
    }
}

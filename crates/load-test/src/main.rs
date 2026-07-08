use bytes::Bytes;
use clap::Parser;
use relay_server::router::StreamFanout;
use teamview_protocol::{
    codec::CodecId,
    control::UserId,
    packet::{MediaPacket, MediaPacketHeader, PacketFlags, PacketType},
};
use tracing::info;

#[derive(Debug, Parser)]
#[command(author, version, about = "Synthetic TeamView relay load test scaffold")]
struct Args {
    #[arg(long, default_value_t = 1)]
    publishers: u16,

    #[arg(long, default_value_t = 10)]
    viewers: u16,

    #[arg(long, default_value_t = 120)]
    packets: u32,

    #[arg(long, default_value_t = 20)]
    media_duration_ms: u16,

    #[arg(long, default_value_t = false)]
    include_slow_viewer: bool,
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let args = Args::parse();
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
            publishers: 1,
            viewers: 4,
            packets: 10,
            media_duration_ms: 20,
            include_slow_viewer: false,
        };

        let result = run_synthetic_fanout(&args);

        assert_eq!(result.total_dropped, 0);
        assert_eq!(result.total_delivered, 40);
    }

    #[test]
    fn synthetic_fanout_isolates_slow_viewer() {
        let args = Args {
            publishers: 1,
            viewers: 4,
            packets: 10,
            media_duration_ms: 20,
            include_slow_viewer: true,
        };

        let result = run_synthetic_fanout(&args);

        assert_eq!(result.slow_viewer_dropped, 9);
        assert_eq!(result.total_dropped, 9);
        assert_eq!(result.total_delivered, 31);
    }
}

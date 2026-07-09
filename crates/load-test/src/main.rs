use std::{sync::Arc, time::Duration};

use anyhow::Context;

use bytes::Bytes;
use clap::{Parser, ValueEnum};
use quinn::{ClientConfig, Connection, Endpoint};
use relay_server::{
    control_stream::ControlRuntime, router::StreamFanout, transport::build_server_endpoint,
};
use teamview_protocol::{
    PROTOCOL_VERSION,
    codec::CodecId,
    control::UserId,
    control::{
        ClientControl, ClientEnvelope, CreateRoom, Hello, JoinRoom, MediaKind, PublishStream,
        ServerControl, SubscribeStream, decode_server_envelope, encode_client_envelope,
    },
    frame::{EncodedFrame, packetize_frame, packetize_frame_for_datagram_target, reassemble_frame},
    packet::{
        DEFAULT_DATAGRAM_PAYLOAD_TARGET, MediaPacket, MediaPacketHeader, PacketFlags, PacketType,
    },
};
use tracing::info;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Mode {
    Fanout,
    SampleForward,
    QuicSampleForward,
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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
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
        Mode::QuicSampleForward => {
            let result = run_quic_sample_forward(&args).await?;
            info!(?args, ?result, "load-test QUIC sample forward complete");
            println!(
                "quic-sample-forward frames={} fragments={} reassembled={} delivered={} dropped={}",
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

async fn run_quic_sample_forward(args: &Args) -> anyhow::Result<SampleForwardResult> {
    let server = build_server_endpoint("127.0.0.1:0")?;
    let server_addr = server.local_addr()?;
    let runtime = ControlRuntime::new();
    let server_task = tokio::spawn(async move {
        runtime.serve_endpoint(server).await;
    });

    let publisher_endpoint = build_client_endpoint()?;
    let publisher = publisher_endpoint
        .connect(server_addr, "localhost")?
        .await
        .context("publisher failed to connect")?;

    let mut viewer_endpoints = Vec::new();
    let mut viewers = Vec::new();
    for _ in 0..args.viewers {
        let endpoint = build_client_endpoint()?;
        let connection = endpoint
            .connect(server_addr, "localhost")?
            .await
            .context("viewer failed to connect")?;
        viewer_endpoints.push(endpoint);
        viewers.push(connection);
    }

    send_control_request(
        &publisher,
        ClientEnvelope::new(
            1,
            ClientControl::Hello(Hello {
                protocol_version: PROTOCOL_VERSION,
                client_name: "load-test-publisher".to_owned(),
            }),
        ),
    )
    .await?;
    let created = send_control_request(
        &publisher,
        ClientEnvelope::new(
            2,
            ClientControl::CreateRoom(CreateRoom {
                name: "load-test".to_owned(),
            }),
        ),
    )
    .await?;
    let room_id = match created.message {
        ServerControl::RoomCreated(room) => room.room_id,
        other => anyhow::bail!("unexpected create room response: {other:?}"),
    };
    send_control_request(
        &publisher,
        ClientEnvelope::new(3, ClientControl::JoinRoom(JoinRoom { room_id })),
    )
    .await?;
    send_control_request(
        &publisher,
        ClientEnvelope::new(
            4,
            ClientControl::PublishStream(PublishStream {
                room_id,
                stream_id: 1,
                codec: CodecId::H264,
                media_kind: MediaKind::Screen,
            }),
        ),
    )
    .await?;

    for (index, viewer) in viewers.iter().enumerate() {
        send_control_request(
            viewer,
            ClientEnvelope::new(
                1,
                ClientControl::Hello(Hello {
                    protocol_version: PROTOCOL_VERSION,
                    client_name: format!("load-test-viewer-{index}"),
                }),
            ),
        )
        .await?;
        send_control_request(
            viewer,
            ClientEnvelope::new(2, ClientControl::JoinRoom(JoinRoom { room_id })),
        )
        .await?;
        send_control_request(
            viewer,
            ClientEnvelope::new(
                3,
                ClientControl::SubscribeStream(SubscribeStream {
                    room_id,
                    stream_id: 1,
                }),
            ),
        )
        .await?;
    }

    let mut result = SampleForwardResult::default();
    for frame_index in 0..args.packets {
        let frame = sample_h264_frame(frame_index + 1);
        let packets = packetize_frame_for_datagram_target(
            &frame,
            frame_index.saturating_mul(100),
            args.max_payload,
        )?;
        result.frames += 1;
        result.fragments += packets.len() as u64;

        for packet in &packets {
            publisher.send_datagram(packet.encode()?)?;
        }

        for viewer in &viewers {
            let mut received = Vec::with_capacity(packets.len());
            for _ in 0..packets.len() {
                let bytes = tokio::time::timeout(Duration::from_secs(2), viewer.read_datagram())
                    .await
                    .context("timed out waiting for forwarded datagram")??;
                received.push(MediaPacket::decode(&bytes)?);
            }
            let reassembled = reassemble_frame(received)?;
            if reassembled.bytes != frame.bytes {
                anyhow::bail!(
                    "QUIC reassembled frame bytes differ for frame {}",
                    frame.frame_id
                );
            }
            result.reassembled_frames += 1;
            result.total_delivered += packets.len() as u64;
        }
    }

    publisher.close(0_u32.into(), b"done");
    for viewer in viewers {
        viewer.close(0_u32.into(), b"done");
    }
    drop(viewer_endpoints);
    drop(publisher_endpoint);
    server_task.abort();

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
        sender_clock_offset_micros: 0,
        sender_encode_done_time_micros: 0,
        sender_send_time_micros: 0,
        server_receive_time_micros: 0,
        server_send_time_micros: 0,
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

fn build_client_endpoint() -> anyhow::Result<Endpoint> {
    let mut endpoint = Endpoint::client("127.0.0.1:0".parse().unwrap())?;
    endpoint.set_default_client_config(build_insecure_local_client_config());
    Ok(endpoint)
}

async fn send_control_request(
    connection: &Connection,
    request: ClientEnvelope,
) -> anyhow::Result<teamview_protocol::control::ServerEnvelope> {
    let (mut send, mut recv) = connection
        .open_bi()
        .await
        .context("failed to open control stream")?;
    send.write_all(&encode_client_envelope(&request)?)
        .await
        .context("failed to write control request")?;
    send.finish().context("failed to finish control request")?;
    let response_bytes = recv
        .read_to_end(64 * 1024)
        .await
        .context("failed to read control response")?;
    Ok(decode_server_envelope(&response_bytes)?)
}

fn build_insecure_local_client_config() -> ClientConfig {
    let mut crypto = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(SkipServerVerification))
        .with_no_client_auth();
    crypto.alpn_protocols = vec![b"teamview-stage1".to_vec()];
    ClientConfig::new(Arc::new(
        quinn::crypto::rustls::QuicClientConfig::try_from(crypto)
            .expect("valid local QUIC client config"),
    ))
}

#[derive(Debug)]
struct SkipServerVerification;

impl rustls::client::danger::ServerCertVerifier for SkipServerVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::ED25519,
            rustls::SignatureScheme::RSA_PSS_SHA256,
        ]
    }
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

    #[tokio::test]
    async fn quic_sample_forward_reassembles_for_each_viewer() {
        let args = Args {
            mode: Mode::QuicSampleForward,
            publishers: 1,
            viewers: 2,
            packets: 2,
            media_duration_ms: 20,
            max_payload: 700,
            include_slow_viewer: false,
        };

        let result = run_quic_sample_forward(&args).await.unwrap();

        assert_eq!(result.frames, 2);
        assert_eq!(result.reassembled_frames, 4);
        assert!(result.fragments > result.frames as u64);
        assert_eq!(result.total_dropped, 0);
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

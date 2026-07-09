use bytes::Bytes;

use crate::{
    codec::CodecId,
    packet::{MEDIA_PACKET_HEADER_LEN, MediaPacket, MediaPacketHeader, PacketFlags, PacketType},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedFrame {
    pub room_stream_id: u32,
    pub frame_id: u32,
    pub media_timestamp: u64,
    pub sender_capture_time_micros: u64,
    pub sender_encode_done_time_micros: u64,
    pub sender_send_time_micros: u64,
    pub server_receive_time_micros: u64,
    pub server_send_time_micros: u64,
    pub codec: CodecId,
    pub is_keyframe: bool,
    pub bytes: Bytes,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FramePacketizeError {
    #[error("max payload must be greater than zero")]
    ZeroMaxPayload,
    #[error("max payload exceeds media packet payload field: {0}")]
    PayloadTooLarge(usize),
    #[error("max datagram payload must be larger than media header")]
    DatagramTooSmall,
    #[error("frame requires too many fragments: {0}")]
    TooManyFragments(usize),
    #[error("packet encode error: {0}")]
    PacketEncode(#[from] crate::packet::PacketEncodeError),
}

pub fn packetize_frame(
    frame: &EncodedFrame,
    first_sequence_number: u32,
    max_payload: usize,
) -> Result<Vec<MediaPacket>, FramePacketizeError> {
    packetize_frame_with_type(frame, PacketType::Video, first_sequence_number, max_payload)
}

pub fn packetize_frame_with_type(
    frame: &EncodedFrame,
    packet_type: PacketType,
    first_sequence_number: u32,
    max_payload: usize,
) -> Result<Vec<MediaPacket>, FramePacketizeError> {
    if max_payload == 0 {
        return Err(FramePacketizeError::ZeroMaxPayload);
    }
    if max_payload > u16::MAX as usize {
        return Err(FramePacketizeError::PayloadTooLarge(max_payload));
    }

    let fragment_count = frame.bytes.len().div_ceil(max_payload).max(1);
    if fragment_count > u16::MAX as usize {
        return Err(FramePacketizeError::TooManyFragments(fragment_count));
    }

    let mut packets = Vec::with_capacity(fragment_count);
    for fragment_index in 0..fragment_count {
        let start = fragment_index * max_payload;
        let end = ((fragment_index + 1) * max_payload).min(frame.bytes.len());
        let payload = if start == end {
            Bytes::new()
        } else {
            frame.bytes.slice(start..end)
        };
        let mut flags = PacketFlags::empty();
        if frame.is_keyframe {
            flags = flags.with(PacketFlags::KEYFRAME);
        }
        if fragment_index + 1 == fragment_count {
            flags = flags.with(PacketFlags::END_OF_FRAME);
        }

        let mut header = MediaPacketHeader::new(
            packet_type,
            frame.codec,
            frame.room_stream_id,
            first_sequence_number.wrapping_add(fragment_index as u32),
            payload.len() as u16,
        );
        header.flags = flags;
        header.frame_id = frame.frame_id;
        header.fragment_index = fragment_index as u16;
        header.fragment_count = fragment_count as u16;
        header.media_timestamp = frame.media_timestamp;
        header.sender_capture_time_micros = frame.sender_capture_time_micros;
        header.sender_encode_done_time_micros = frame.sender_encode_done_time_micros;
        header.sender_send_time_micros = frame.sender_send_time_micros;
        header.server_receive_time_micros = frame.server_receive_time_micros;
        header.server_send_time_micros = frame.server_send_time_micros;

        let packet = MediaPacket { header, payload };
        packet.encode()?;
        packets.push(packet);
    }

    Ok(packets)
}

pub fn packetize_frame_for_datagram_target(
    frame: &EncodedFrame,
    first_sequence_number: u32,
    max_datagram_payload: usize,
) -> Result<Vec<MediaPacket>, FramePacketizeError> {
    if max_datagram_payload <= MEDIA_PACKET_HEADER_LEN {
        return Err(FramePacketizeError::DatagramTooSmall);
    }
    packetize_frame_with_type_for_datagram_target(
        frame,
        PacketType::Video,
        first_sequence_number,
        max_datagram_payload,
    )
}

pub fn packetize_frame_with_type_for_datagram_target(
    frame: &EncodedFrame,
    packet_type: PacketType,
    first_sequence_number: u32,
    max_datagram_payload: usize,
) -> Result<Vec<MediaPacket>, FramePacketizeError> {
    if max_datagram_payload <= MEDIA_PACKET_HEADER_LEN {
        return Err(FramePacketizeError::DatagramTooSmall);
    }
    packetize_frame_with_type(
        frame,
        packet_type,
        first_sequence_number,
        max_datagram_payload - MEDIA_PACKET_HEADER_LEN,
    )
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FrameReassemblyError {
    #[error("no packets supplied")]
    Empty,
    #[error("missing fragment {0}")]
    MissingFragment(u16),
    #[error("duplicate fragment {0}")]
    DuplicateFragment(u16),
    #[error("fragment metadata mismatch")]
    FragmentMetadataMismatch,
    #[error("fragment count mismatch")]
    FragmentCountMismatch,
    #[error("invalid end-of-frame flag on fragment {0}")]
    InvalidEndOfFrameFlag(u16),
    #[error("packet header invalid: {0}")]
    InvalidPacketHeader(#[from] crate::packet::PacketDecodeError),
}

pub fn reassemble_frame(
    mut packets: Vec<MediaPacket>,
) -> Result<EncodedFrame, FrameReassemblyError> {
    if packets.is_empty() {
        return Err(FrameReassemblyError::Empty);
    }

    for packet in &packets {
        packet.header.validate()?;
    }

    packets.sort_by_key(|packet| packet.header.fragment_index);
    let first = packets[0].header.clone();
    if !matches!(first.packet_type, PacketType::Video | PacketType::Audio) {
        return Err(FrameReassemblyError::FragmentMetadataMismatch);
    }
    if packets.len() != first.fragment_count as usize {
        return Err(FrameReassemblyError::FragmentCountMismatch);
    }

    let is_keyframe = first.flags.contains(PacketFlags::KEYFRAME);
    let mut sender_encode_done_time_micros = first.sender_encode_done_time_micros;
    let mut sender_send_time_micros = first.sender_send_time_micros;
    let mut server_receive_time_micros = min_nonzero(first.server_receive_time_micros, 0);
    let mut server_send_time_micros = first.server_send_time_micros;
    let mut bytes = Vec::new();
    let mut previous_fragment_index = None;
    for (expected_index, packet) in packets.iter().enumerate() {
        let fragment_index = packet.header.fragment_index;
        if previous_fragment_index == Some(fragment_index) {
            return Err(FrameReassemblyError::DuplicateFragment(fragment_index));
        }
        previous_fragment_index = Some(fragment_index);

        if fragment_index != expected_index as u16 {
            return Err(FrameReassemblyError::MissingFragment(expected_index as u16));
        }
        if packet.header.fragment_count != first.fragment_count
            || packet.header.frame_id != first.frame_id
            || packet.header.room_stream_id != first.room_stream_id
            || packet.header.codec != first.codec
            || packet.header.packet_type != first.packet_type
            || packet.header.layer != first.layer
            || packet.header.media_timestamp != first.media_timestamp
            || packet.header.sender_capture_time_micros != first.sender_capture_time_micros
            || packet.header.flags.contains(PacketFlags::KEYFRAME) != is_keyframe
        {
            return Err(FrameReassemblyError::FragmentMetadataMismatch);
        }
        sender_encode_done_time_micros =
            sender_encode_done_time_micros.max(packet.header.sender_encode_done_time_micros);
        sender_send_time_micros =
            sender_send_time_micros.max(packet.header.sender_send_time_micros);
        server_receive_time_micros = min_nonzero(
            server_receive_time_micros,
            packet.header.server_receive_time_micros,
        );
        server_send_time_micros =
            server_send_time_micros.max(packet.header.server_send_time_micros);

        let expected_end_of_frame = expected_index + 1 == first.fragment_count as usize;
        if packet.header.flags.contains(PacketFlags::END_OF_FRAME) != expected_end_of_frame {
            return Err(FrameReassemblyError::InvalidEndOfFrameFlag(fragment_index));
        }

        bytes.extend_from_slice(&packet.payload);
    }

    Ok(EncodedFrame {
        room_stream_id: first.room_stream_id,
        frame_id: first.frame_id,
        media_timestamp: first.media_timestamp,
        sender_capture_time_micros: first.sender_capture_time_micros,
        sender_encode_done_time_micros,
        sender_send_time_micros,
        server_receive_time_micros,
        server_send_time_micros,
        codec: first.codec,
        is_keyframe,
        bytes: Bytes::from(bytes),
    })
}

fn min_nonzero(left: u64, right: u64) -> u64 {
    match (left, right) {
        (0, value) | (value, 0) => value,
        (left, right) => left.min(right),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packetizes_and_reassembles_multifragment_frame() {
        let frame = sample_frame(64);
        let packets = packetize_frame(&frame, 100, 17).unwrap();

        assert_eq!(packets.len(), 4);
        assert_eq!(packets[0].header.sequence_number, 100);
        assert_eq!(packets[3].header.sequence_number, 103);
        assert!(packets[0].header.flags.contains(PacketFlags::KEYFRAME));
        assert!(!packets[0].header.flags.contains(PacketFlags::END_OF_FRAME));
        assert!(packets[3].header.flags.contains(PacketFlags::END_OF_FRAME));

        let reassembled = reassemble_frame(packets).unwrap();
        assert_eq!(reassembled, frame);
    }

    #[test]
    fn packetizes_for_datagram_target() {
        let frame = sample_frame(64);
        let packets =
            packetize_frame_for_datagram_target(&frame, 1, MEDIA_PACKET_HEADER_LEN + 10).unwrap();

        assert_eq!(packets.len(), 7);
        for packet in packets {
            assert!(packet.encode().unwrap().len() <= MEDIA_PACKET_HEADER_LEN + 10);
        }
    }

    #[test]
    fn packetizes_audio_frame_with_audio_packet_type() {
        let frame = EncodedFrame {
            room_stream_id: 9,
            frame_id: 7,
            media_timestamp: 960,
            sender_capture_time_micros: 1_234_567,
            sender_encode_done_time_micros: 0,
            sender_send_time_micros: 0,
            server_receive_time_micros: 0,
            server_send_time_micros: 0,
            codec: CodecId::Opus,
            is_keyframe: false,
            bytes: Bytes::from_static(b"synthetic-opus"),
        };

        let packets = packetize_frame_with_type(&frame, PacketType::Audio, 1, 8).unwrap();

        assert_eq!(packets[0].header.packet_type, PacketType::Audio);
        let reassembled = reassemble_frame(packets).unwrap();
        assert_eq!(reassembled, frame);
    }

    #[test]
    fn packetizes_zero_length_frame() {
        let frame = sample_frame(0);
        let packets = packetize_frame(&frame, 1, 10).unwrap();

        assert_eq!(packets.len(), 1);
        assert_eq!(packets[0].payload.len(), 0);
        assert!(packets[0].header.flags.contains(PacketFlags::END_OF_FRAME));
    }

    #[test]
    fn sequence_numbers_wrap() {
        let frame = sample_frame(30);
        let packets = packetize_frame(&frame, u32::MAX - 1, 10).unwrap();

        assert_eq!(packets[0].header.sequence_number, u32::MAX - 1);
        assert_eq!(packets[1].header.sequence_number, u32::MAX);
        assert_eq!(packets[2].header.sequence_number, 0);
    }

    #[test]
    fn reassembly_rejects_missing_fragment() {
        let frame = sample_frame(64);
        let mut packets = packetize_frame(&frame, 1, 17).unwrap();
        packets.remove(1);

        assert_eq!(
            reassemble_frame(packets),
            Err(FrameReassemblyError::FragmentCountMismatch)
        );
    }

    #[test]
    fn reassembly_rejects_duplicate_fragment() {
        let frame = sample_frame(64);
        let mut packets = packetize_frame(&frame, 1, 17).unwrap();
        packets[1] = packets[0].clone();

        assert_eq!(
            reassemble_frame(packets),
            Err(FrameReassemblyError::DuplicateFragment(0))
        );
    }

    #[test]
    fn reassembly_rejects_mixed_timestamp() {
        let frame = sample_frame(64);
        let mut packets = packetize_frame(&frame, 1, 17).unwrap();
        packets[1].header.media_timestamp += 1;

        assert_eq!(
            reassemble_frame(packets),
            Err(FrameReassemblyError::FragmentMetadataMismatch)
        );
    }

    #[test]
    fn reassembly_preserves_relay_timestamp_span() {
        let frame = sample_frame(64);
        let mut packets = packetize_frame(&frame, 1, 17).unwrap();
        packets[0].header.sender_encode_done_time_micros = 800;
        packets[0].header.sender_send_time_micros = 850;
        packets[1].header.sender_encode_done_time_micros = 800;
        packets[1].header.sender_send_time_micros = 875;
        packets[0].header.server_receive_time_micros = 1_000;
        packets[0].header.server_send_time_micros = 1_500;
        packets[1].header.server_receive_time_micros = 900;
        packets[1].header.server_send_time_micros = 1_700;

        let reassembled = reassemble_frame(packets).unwrap();

        assert_eq!(reassembled.sender_encode_done_time_micros, 800);
        assert_eq!(reassembled.sender_send_time_micros, 875);
        assert_eq!(reassembled.server_receive_time_micros, 900);
        assert_eq!(reassembled.server_send_time_micros, 1_700);
    }

    #[test]
    fn reassembly_rejects_bad_end_of_frame_flag() {
        let frame = sample_frame(64);
        let mut packets = packetize_frame(&frame, 1, 17).unwrap();
        packets[0].header.flags = packets[0].header.flags.with(PacketFlags::END_OF_FRAME);

        assert_eq!(
            reassemble_frame(packets),
            Err(FrameReassemblyError::InvalidEndOfFrameFlag(0))
        );
    }

    fn sample_frame(len: usize) -> EncodedFrame {
        let bytes = (0..len).map(|idx| idx as u8).collect::<Vec<_>>();
        EncodedFrame {
            room_stream_id: 9,
            frame_id: 7,
            media_timestamp: 90_000,
            sender_capture_time_micros: 1_234_567,
            sender_encode_done_time_micros: 0,
            sender_send_time_micros: 0,
            server_receive_time_micros: 0,
            server_send_time_micros: 0,
            codec: CodecId::H264,
            is_keyframe: true,
            bytes: Bytes::from(bytes),
        }
    }
}

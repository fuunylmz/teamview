use bytes::{Buf, BufMut, Bytes, BytesMut};
use serde::{Deserialize, Serialize};

use crate::{PROTOCOL_VERSION, codec::CodecId};

pub const MEDIA_PACKET_HEADER_LEN: usize = 81;
pub const DEFAULT_DATAGRAM_PAYLOAD_TARGET: usize = 1_150;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum PacketType {
    Video = 1,
    Audio = 2,
    Probe = 3,
}

impl TryFrom<u8> for PacketType {
    type Error = PacketDecodeError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::Video),
            2 => Ok(Self::Audio),
            3 => Ok(Self::Probe),
            other => Err(PacketDecodeError::UnknownPacketType(other)),
        }
    }
}

impl From<PacketType> for u8 {
    fn from(value: PacketType) -> Self {
        value as u8
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PacketFlags(u8);

impl PacketFlags {
    pub const KEYFRAME: u8 = 0b0000_0001;
    pub const END_OF_FRAME: u8 = 0b0000_0010;
    pub const CONFIG: u8 = 0b0000_0100;
    pub const DISCARDABLE: u8 = 0b0000_1000;

    pub fn empty() -> Self {
        Self(0)
    }

    pub fn from_bits(bits: u8) -> Self {
        Self(bits)
    }

    pub fn bits(self) -> u8 {
        self.0
    }

    pub fn contains(self, flag: u8) -> bool {
        self.0 & flag != 0
    }

    pub fn with(mut self, flag: u8) -> Self {
        self.0 |= flag;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaPacketHeader {
    pub version: u8,
    pub packet_type: PacketType,
    pub flags: PacketFlags,
    pub room_stream_id: u32,
    pub sequence_number: u32,
    pub frame_id: u32,
    pub fragment_index: u16,
    pub fragment_count: u16,
    pub media_timestamp: u64,
    pub sender_capture_time_micros: u64,
    pub sender_clock_offset_micros: i64,
    pub sender_encode_done_time_micros: u64,
    pub sender_send_time_micros: u64,
    pub server_receive_time_micros: u64,
    pub server_send_time_micros: u64,
    pub codec: CodecId,
    pub layer: u8,
    pub payload_length: u16,
}

impl MediaPacketHeader {
    pub fn new(
        packet_type: PacketType,
        codec: CodecId,
        room_stream_id: u32,
        sequence_number: u32,
        payload_length: u16,
    ) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            packet_type,
            flags: PacketFlags::empty(),
            room_stream_id,
            sequence_number,
            frame_id: 0,
            fragment_index: 0,
            fragment_count: 1,
            media_timestamp: 0,
            sender_capture_time_micros: 0,
            sender_clock_offset_micros: 0,
            sender_encode_done_time_micros: 0,
            sender_send_time_micros: 0,
            server_receive_time_micros: 0,
            server_send_time_micros: 0,
            codec,
            layer: 0,
            payload_length,
        }
    }

    pub fn encode(&self, dst: &mut BytesMut) {
        dst.put_u8(self.version);
        dst.put_u8(self.packet_type.into());
        dst.put_u8(self.flags.bits());
        dst.put_u16(MEDIA_PACKET_HEADER_LEN as u16);
        dst.put_u32(self.room_stream_id);
        dst.put_u32(self.sequence_number);
        dst.put_u32(self.frame_id);
        dst.put_u16(self.fragment_index);
        dst.put_u16(self.fragment_count);
        dst.put_u64(self.media_timestamp);
        dst.put_u64(self.sender_capture_time_micros);
        dst.put_i64(self.sender_clock_offset_micros);
        dst.put_u64(self.sender_encode_done_time_micros);
        dst.put_u64(self.sender_send_time_micros);
        dst.put_u64(self.server_receive_time_micros);
        dst.put_u64(self.server_send_time_micros);
        dst.put_u8(self.codec.into());
        dst.put_u8(self.layer);
        dst.put_u16(self.payload_length);
    }

    pub fn decode(src: &[u8]) -> Result<DecodedMediaPacketHeader, PacketDecodeError> {
        if src.len() < MEDIA_PACKET_HEADER_LEN {
            return Err(PacketDecodeError::TooShort {
                expected: MEDIA_PACKET_HEADER_LEN,
                actual: src.len(),
            });
        }

        let mut buf = src;
        let version = buf.get_u8();
        if version != PROTOCOL_VERSION {
            return Err(PacketDecodeError::UnsupportedVersion(version));
        }

        let packet_type = PacketType::try_from(buf.get_u8())?;
        let flags = PacketFlags::from_bits(buf.get_u8());
        let header_length = buf.get_u16() as usize;
        if header_length < MEDIA_PACKET_HEADER_LEN {
            return Err(PacketDecodeError::UnsupportedHeaderLength(header_length));
        }
        if src.len() < header_length {
            return Err(PacketDecodeError::TooShort {
                expected: header_length,
                actual: src.len(),
            });
        }

        let room_stream_id = buf.get_u32();
        let sequence_number = buf.get_u32();
        let frame_id = buf.get_u32();
        let fragment_index = buf.get_u16();
        let fragment_count = buf.get_u16();
        let media_timestamp = buf.get_u64();
        let sender_capture_time_micros = buf.get_u64();
        let sender_clock_offset_micros = buf.get_i64();
        let sender_encode_done_time_micros = buf.get_u64();
        let sender_send_time_micros = buf.get_u64();
        let server_receive_time_micros = buf.get_u64();
        let server_send_time_micros = buf.get_u64();
        let codec = CodecId::try_from(buf.get_u8())
            .map_err(|err| PacketDecodeError::UnknownCodec(err.0))?;
        let layer = buf.get_u8();
        let payload_length = buf.get_u16();

        let header = Self {
            version,
            packet_type,
            flags,
            room_stream_id,
            sequence_number,
            frame_id,
            fragment_index,
            fragment_count,
            media_timestamp,
            sender_capture_time_micros,
            sender_clock_offset_micros,
            sender_encode_done_time_micros,
            sender_send_time_micros,
            server_receive_time_micros,
            server_send_time_micros,
            codec,
            layer,
            payload_length,
        };
        header.validate()?;

        Ok(DecodedMediaPacketHeader {
            header,
            payload_offset: header_length,
        })
    }

    pub fn validate(&self) -> Result<(), PacketDecodeError> {
        if self.fragment_count == 0 {
            return Err(PacketDecodeError::InvalidFragment {
                fragment_index: self.fragment_index,
                fragment_count: self.fragment_count,
            });
        }
        if self.fragment_index >= self.fragment_count {
            return Err(PacketDecodeError::InvalidFragment {
                fragment_index: self.fragment_index,
                fragment_count: self.fragment_count,
            });
        }
        if self.layer != 0 {
            return Err(PacketDecodeError::UnsupportedLayer(self.layer));
        }
        if self.packet_type == PacketType::Probe && self.codec != CodecId::H264 {
            return Err(PacketDecodeError::InvalidPacketCodec {
                packet_type: self.packet_type,
                codec: self.codec,
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedMediaPacketHeader {
    pub header: MediaPacketHeader,
    pub payload_offset: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaPacket {
    pub header: MediaPacketHeader,
    pub payload: Bytes,
}

impl MediaPacket {
    pub fn encode(&self) -> Result<Bytes, PacketEncodeError> {
        if self.payload.len() > u16::MAX as usize {
            return Err(PacketEncodeError::PayloadTooLarge(self.payload.len()));
        }
        if self.header.payload_length as usize != self.payload.len() {
            return Err(PacketEncodeError::PayloadLengthMismatch {
                header: self.header.payload_length as usize,
                actual: self.payload.len(),
            });
        }
        self.header.validate()?;

        let mut dst = BytesMut::with_capacity(MEDIA_PACKET_HEADER_LEN + self.payload.len());
        self.header.encode(&mut dst);
        dst.extend_from_slice(&self.payload);
        Ok(dst.freeze())
    }

    pub fn decode(src: &[u8]) -> Result<Self, PacketDecodeError> {
        let decoded = MediaPacketHeader::decode(src)?;
        let expected_len = decoded.payload_offset + decoded.header.payload_length as usize;
        if src.len() < expected_len {
            return Err(PacketDecodeError::PayloadTooShort {
                expected: expected_len,
                actual: src.len(),
            });
        }
        if src.len() > expected_len {
            return Err(PacketDecodeError::TrailingBytes {
                expected: expected_len,
                actual: src.len(),
            });
        }

        Ok(Self {
            header: decoded.header,
            payload: Bytes::copy_from_slice(&src[decoded.payload_offset..expected_len]),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PacketDecodeError {
    #[error("packet header too short: expected at least {expected} bytes, got {actual}")]
    TooShort { expected: usize, actual: usize },
    #[error("packet payload too short: expected at least {expected} bytes, got {actual}")]
    PayloadTooShort { expected: usize, actual: usize },
    #[error("packet has trailing bytes: expected exactly {expected} bytes, got {actual}")]
    TrailingBytes { expected: usize, actual: usize },
    #[error("unsupported protocol version {0}")]
    UnsupportedVersion(u8),
    #[error("unsupported header length {0}")]
    UnsupportedHeaderLength(usize),
    #[error("unknown packet type {0}")]
    UnknownPacketType(u8),
    #[error("unknown codec id {0}")]
    UnknownCodec(u8),
    #[error("invalid fragment {fragment_index}/{fragment_count}")]
    InvalidFragment {
        fragment_index: u16,
        fragment_count: u16,
    },
    #[error("unsupported layer {0}")]
    UnsupportedLayer(u8),
    #[error("packet type {packet_type:?} cannot use codec {codec:?}")]
    InvalidPacketCodec {
        packet_type: PacketType,
        codec: CodecId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PacketEncodeError {
    #[error("payload too large for u16 length field: {0} bytes")]
    PayloadTooLarge(usize),
    #[error(
        "payload length mismatch: header says {header} bytes, actual payload is {actual} bytes"
    )]
    PayloadLengthMismatch { header: usize, actual: usize },
    #[error(transparent)]
    InvalidHeader(#[from] PacketDecodeError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_header_round_trips() {
        let header = MediaPacketHeader {
            version: PROTOCOL_VERSION,
            packet_type: PacketType::Video,
            flags: PacketFlags::empty()
                .with(PacketFlags::KEYFRAME)
                .with(PacketFlags::END_OF_FRAME),
            room_stream_id: 42,
            sequence_number: 7,
            frame_id: 3,
            fragment_index: 1,
            fragment_count: 2,
            media_timestamp: 123_456,
            sender_capture_time_micros: 654_321,
            sender_clock_offset_micros: -123,
            sender_encode_done_time_micros: 655_000,
            sender_send_time_micros: 656_000,
            server_receive_time_micros: 777_000,
            server_send_time_micros: 778_000,
            codec: CodecId::H264,
            layer: 0,
            payload_length: 4,
        };

        let mut encoded = BytesMut::new();
        header.encode(&mut encoded);

        assert_eq!(encoded.len(), MEDIA_PACKET_HEADER_LEN);
        assert_eq!(MediaPacketHeader::decode(&encoded).unwrap().header, header);
    }

    #[test]
    fn media_packet_round_trips() {
        let payload = Bytes::from_static(b"data");
        let packet = MediaPacket {
            header: MediaPacketHeader::new(
                PacketType::Video,
                CodecId::H264,
                1,
                9,
                payload.len() as u16,
            ),
            payload,
        };

        let encoded = packet.encode().unwrap();
        let decoded = MediaPacket::decode(&encoded).unwrap();

        assert_eq!(decoded, packet);
    }

    #[test]
    fn rejects_unknown_version() {
        let mut encoded = BytesMut::new();
        MediaPacketHeader::new(PacketType::Video, CodecId::H264, 1, 1, 0).encode(&mut encoded);
        encoded[0] = PROTOCOL_VERSION + 1;

        assert_eq!(
            MediaPacketHeader::decode(&encoded),
            Err(PacketDecodeError::UnsupportedVersion(PROTOCOL_VERSION + 1))
        );
    }

    #[test]
    fn skips_forward_compatible_header_extensions() {
        let payload = Bytes::from_static(b"data");
        let packet = MediaPacket {
            header: MediaPacketHeader::new(
                PacketType::Video,
                CodecId::H264,
                1,
                9,
                payload.len() as u16,
            ),
            payload,
        };
        let mut encoded = BytesMut::from(&packet.encode().unwrap()[..MEDIA_PACKET_HEADER_LEN]);
        encoded[3] = 0;
        encoded[4] = (MEDIA_PACKET_HEADER_LEN + 2) as u8;
        encoded.extend_from_slice(&[0xaa, 0xbb]);
        encoded.extend_from_slice(&packet.payload);

        let decoded = MediaPacket::decode(&encoded).unwrap();

        assert_eq!(decoded.header, packet.header);
        assert_eq!(decoded.payload, packet.payload);
    }

    #[test]
    fn rejects_invalid_fragment_count() {
        let mut header = MediaPacketHeader::new(PacketType::Video, CodecId::H264, 1, 1, 0);
        header.fragment_count = 0;

        assert_eq!(
            header.validate(),
            Err(PacketDecodeError::InvalidFragment {
                fragment_index: 0,
                fragment_count: 0
            })
        );
    }

    #[test]
    fn rejects_payload_length_mismatch_on_encode() {
        let packet = MediaPacket {
            header: MediaPacketHeader::new(PacketType::Video, CodecId::H264, 1, 1, 99),
            payload: Bytes::from_static(b"data"),
        };

        assert_eq!(
            packet.encode(),
            Err(PacketEncodeError::PayloadLengthMismatch {
                header: 99,
                actual: 4
            })
        );
    }
}

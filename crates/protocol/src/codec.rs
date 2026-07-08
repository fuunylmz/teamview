use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum CodecId {
    H264 = 1,
    Av1 = 2,
    Hevc = 3,
    Opus = 16,
}

impl TryFrom<u8> for CodecId {
    type Error = UnknownCodec;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            1 => Ok(Self::H264),
            2 => Ok(Self::Av1),
            3 => Ok(Self::Hevc),
            16 => Ok(Self::Opus),
            other => Err(UnknownCodec(other)),
        }
    }
}

impl From<CodecId> for u8 {
    fn from(value: CodecId) -> Self {
        value as u8
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("unknown codec id {0}")]
pub struct UnknownCodec(pub u8);

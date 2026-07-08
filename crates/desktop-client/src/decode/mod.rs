pub mod h264;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedFrame {
    pub width: u32,
    pub height: u32,
    pub render_time_micros: u64,
}

pub trait VideoDecoder {
    fn decode(&mut self, encoded: &[u8]) -> anyhow::Result<Option<DecodedFrame>>;
}

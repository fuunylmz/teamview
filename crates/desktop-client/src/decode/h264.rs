use super::{DecodedFrame, VideoDecoder};

#[derive(Debug, Default)]
pub struct H264Decoder;

impl VideoDecoder for H264Decoder {
    fn decode(&mut self, _encoded: &[u8]) -> anyhow::Result<Option<DecodedFrame>> {
        Ok(None)
    }
}

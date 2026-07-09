use super::{DecodedFrame, VideoDecoder};

#[derive(Debug, Default)]
pub struct H264Decoder;

impl VideoDecoder for H264Decoder {
    fn decode(&mut self, encoded: &[u8]) -> anyhow::Result<Option<DecodedFrame>> {
        if encoded.is_empty() {
            return Ok(None);
        }

        Ok(Some(DecodedFrame {
            width: 1280,
            height: 720,
            render_time_micros: 0,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthetic_decoder_outputs_frame_for_non_empty_input() {
        let mut decoder = H264Decoder;

        let decoded = decoder.decode(b"synthetic-h264").unwrap();

        assert_eq!(
            decoded,
            Some(DecodedFrame {
                width: 1280,
                height: 720,
                render_time_micros: 0,
            })
        );
    }

    #[test]
    fn synthetic_decoder_waits_on_empty_input() {
        let mut decoder = H264Decoder;

        assert_eq!(decoder.decode(b"").unwrap(), None);
    }
}

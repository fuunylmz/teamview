use crate::decode::DecodedFrame;

pub trait VideoPlayback {
    fn render(&mut self, frame: DecodedFrame) -> anyhow::Result<()>;
}

#[derive(Debug, Default)]
pub struct NullPlayback;

impl VideoPlayback for NullPlayback {
    fn render(&mut self, _frame: DecodedFrame) -> anyhow::Result<()> {
        Ok(())
    }
}

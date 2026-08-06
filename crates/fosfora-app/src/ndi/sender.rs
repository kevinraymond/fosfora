use super::ffi::NdiSender;
use crate::output::sink::{FrameSink, OutputFrame};

/// NDI frame writer, running on the shared output sender thread.
pub struct NdiSink {
    sender: NdiSender,
}

impl NdiSink {
    /// Load the NDI runtime and create a sender. Constructed inside the sender
    /// thread so the dylib load never blocks the render thread.
    pub fn new(source_name: &str) -> Result<Self, String> {
        Ok(Self {
            sender: NdiSender::new(source_name)?,
        })
    }
}

impl FrameSink for NdiSink {
    fn write_frame(&mut self, frame: &OutputFrame) -> Result<(), String> {
        self.sender
            .send_video(&frame.data, frame.width, frame.height, frame.layout);
        Ok(())
    }
}

//! Shared plumbing for CPU-readback video output sinks (NDI, v4l2, Spout, Syphon).
//!
//! Each sink pairs its own config + panel with an [`pipeline::OutputPipeline`], which
//! owns the GPU capture target, the frame channel, and the sender thread. The per-sink
//! writer implements [`sink::FrameSink`] and runs on that thread.

pub mod pipeline;
pub mod sink;

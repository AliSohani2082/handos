//! Wire protocol shared by the daemon, tracker and trace files.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod control;
mod frame;
pub mod trace;

pub use frame::{Hand, HandFrame, Handedness, decode_frame, encode_frame};

pub const PROTOCOL: u8 = 1;
pub const MSG_FRAME: u8 = 1;
pub const MSG_CONTROL: u8 = 2;
pub const FLAG_WORLD: u8 = 1;

#[derive(Debug, thiserror::Error)]
pub enum ProtoError {
    #[error("unexpected message type")]
    MessageType,
    #[error("unsupported HandFrame protocol version {0}")]
    Version(u8),
    #[error("invalid HandFrame flags {0:#x}")]
    Flags(u8),
    #[error("HandFrame has {0} hands, maximum is 4")]
    HandCount(usize),
    #[error("invalid packet length")]
    Length,
    #[error("invalid handedness {0}")]
    Handedness(u8),
    #[error("HandFrame contains a non-finite value")]
    NonFinite,
    #[error("world landmarks do not match the HandFrame flags")]
    World,
    #[error("truncated trace")]
    Truncated,
    #[error("invalid or unsupported trace header")]
    TraceHeader,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

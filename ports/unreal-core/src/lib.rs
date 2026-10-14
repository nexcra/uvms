//! Partial native port. See README.md for exact evidence and unsupported behavior.
//! No RTSP, muxer, WebRTC engine, WebSocket transport, or codec implementation.

pub mod frame;
pub mod mse;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    EmptyFrame,
    TruncatedLength,
    InvalidNalLength,
    TooManyNals,
    AmbiguousOneByteNal,
    InvalidParameterSets,
    TooLarge,
    InvalidTimeshiftTime,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for Error {}

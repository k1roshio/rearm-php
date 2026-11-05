use thiserror::Error;

/// Errors produced while encoding or decoding protocol frames.
#[derive(Debug, Error)]
pub enum ProtocolError {
    #[error("messagepack encode error: {0}")]
    Encode(#[from] rmp_serde::encode::Error),

    #[error("messagepack decode error: {0}")]
    Decode(#[from] rmp_serde::decode::Error),

    #[error("frame too short: need at least 4 bytes for length prefix")]
    TruncatedHeader,

    #[error("incomplete frame: have {have} bytes, need {need}")]
    Incomplete { have: usize, need: usize },

    #[error("frame length {0} exceeds maximum allowed size")]
    FrameTooLarge(u32),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

//! Shared error types.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ZalkError {
    #[error("payload too large: {declared} > {max}")]
    PayloadTooLarge { declared: u32, max: u32 },

    #[error("invalid magic bytes")]
    InvalidMagic,

    #[error("unknown protocol version: {0}")]
    UnknownVersion(u8),

    #[error("unknown message type: {0}")]
    UnknownMessageType(u8),

    #[error("trailing bytes after message")]
    TrailingBytes,

    #[error("truncated message")]
    Truncated,

    #[error("invalid code hash: expected {expected}, got {actual}")]
    CodeHashMismatch { expected: String, actual: String },

    #[error("missing carrier chunk {0}")]
    MissingChunk(u8),

    #[error("duplicate carrier chunk {0}")]
    DuplicateChunk(u8),

    #[error("chunk index {0} out of range (count={1})")]
    ChunkOutOfRange(u8, u8),

    #[error("contract not found: {0}")]
    ContractNotFound(String),

    #[error("call depth exceeded ({0} > {1})")]
    CallDepthExceeded(u32, u32),

    #[error("fuel exhausted")]
    FuelExhausted,

    #[error("WASM trap: {0}")]
    WasmTrap(String),

    #[error("storage key too large")]
    StorageKeyTooLarge,

    #[error("storage value too large")]
    StorageValueTooLarge,

    #[error("invalid WASM module: {0}")]
    InvalidWasm(String),

    #[error("chain source error: {0}")]
    ChainSource(String),

    #[error("state error: {0}")]
    State(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

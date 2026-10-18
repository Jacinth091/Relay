//! Typed errors returned by Herdr CLI operations.

use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum HerdrClientError {
    #[error("transport error: {0}")]
    Transport(String),
    #[error("Herdr executable not found: {0}")]
    ExecutableNotFound(String),
    #[error("Herdr command error ({code}): {message}")]
    CommandError { code: String, message: String },
    #[error(
        "incompatible protocol version: server reported {server_version}, expected minimum {expected_min}"
    )]
    IncompatibleVersion {
        server_version: String,
        expected_min: String,
    },
    #[error("resource not found: {0}")]
    ResourceNotFound(String),
    #[error("RPC call timed out: {0}")]
    Timeout(String),
}

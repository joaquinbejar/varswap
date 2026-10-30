//! Posting errors.

/// Why a post failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PostError {
    /// A Hermes binary update (accumulator update, VAA or price message) cannot be decoded or
    /// planned. A fixed category; the bytes are never quoted.
    #[error("price update cannot be decoded: {0}")]
    Update(&'static str),
    /// The update source failed.
    #[error("update source failed: {0}")]
    Source(String),
    /// A chain operation (read, send) failed. Carries redacted text only.
    #[error("chain operation failed: {0}")]
    Chain(String),
}

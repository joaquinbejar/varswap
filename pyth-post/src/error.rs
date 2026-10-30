//! Posting errors.

use solana_address::Address;

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
    /// The update source answered 429 (rate limited): back off and retry.
    #[error("update source rate limit (HTTP 429): back off and retry")]
    RateLimited,
    /// The post failed (`cause`) and its encoded VAA could not be closed: it is still open at
    /// `encoded_vaa`, holding its rent. Close it later with [`crate::close_if_open`].
    #[error("{cause}; encoded VAA {encoded_vaa} is still open")]
    Orphaned {
        /// The open encoded VAA account.
        encoded_vaa: Address,
        /// The failure of the post, as text.
        cause: String,
    },
}

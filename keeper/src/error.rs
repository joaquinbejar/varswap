//! Keeper errors.

/// Why a keeper step failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KeeperError {
    /// The window account is missing or cannot be decoded.
    #[error("window account cannot be decoded: {0}")]
    Decode(&'static str),
    /// The stored window is rejected by `varswap-core`.
    #[error("window rejected: {0:?}")]
    Core(varswap_core::CoreError),
    /// The price source failed.
    #[error("price source failed: {0}")]
    Source(String),
    /// A price-source response could not be used. Carries only the HTTP status, the body
    /// length and a fixed category, never response content (which could reflect the API key).
    #[error("malformed price-source response (HTTP {status}, {byte_len} bytes): {what}")]
    MalformedResponse {
        /// HTTP status of the response.
        status: u16,
        /// Length of the body, bytes.
        byte_len: usize,
        /// What was wrong, from a fixed set of descriptions.
        what: &'static str,
    },
    /// A Hermes binary update (accumulator update, VAA or price message) cannot be decoded.
    /// A fixed category; the bytes are never quoted.
    #[error("price update cannot be decoded: {0}")]
    Update(&'static str),
    /// The price source answered 429: the key's rate limit (about 10 requests a minute on
    /// Hermes' timestamp endpoint, `retry-after` 60). Back off and retry.
    #[error("price source rate limit (HTTP 429): back off and retry")]
    RateLimited,
    /// A chain operation (read, post, finalize) failed.
    #[error("chain operation failed: {0}")]
    Chain(String),
    /// A configuration value is missing or invalid.
    #[error("configuration: {0}")]
    Config(&'static str),
}

impl From<pyth_post::PostError> for KeeperError {
    fn from(e: pyth_post::PostError) -> Self {
        match e {
            pyth_post::PostError::Update(what) => Self::Update(what),
            pyth_post::PostError::Source(msg) => Self::Source(msg),
            pyth_post::PostError::Chain(msg) => Self::Chain(msg),
        }
    }
}

impl From<varswap_core::CoreError> for KeeperError {
    fn from(e: varswap_core::CoreError) -> Self {
        Self::Core(e)
    }
}

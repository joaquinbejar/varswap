//! CLI errors. None carries the RPC URL (providers put API keys in it) or key material.

/// Everything that can go wrong in a CLI command.
#[derive(Debug, thiserror::Error)]
pub enum CliError {
    /// A bad argument value (the text is built locally from the argument, never from a server).
    #[error("{0}")]
    Usage(String),
    /// The keypair file could not be read or is not a Solana keypair.
    #[error("keypair file {path}: {what}")]
    Keypair {
        /// The file path.
        path: String,
        /// What went wrong, never the file's content.
        what: &'static str,
    },
    /// The RPC call failed, or the transaction was rejected or failed; preflight failures carry
    /// the program logs. Everything from the node is untrusted text, redacted as a whole.
    #[error("RPC: {0}")]
    Rpc(String),
    /// A required account does not exist.
    #[error("account {0} not found")]
    AccountNotFound(String),
    /// An account exists but is not the expected varswap account.
    #[error("account {address} is not a varswap {what}")]
    Decode {
        /// The account address.
        address: String,
        /// The expected account kind.
        what: &'static str,
    },
    /// Signing the transaction failed.
    #[error("signing: {0}")]
    Sign(&'static str),
}

//! The cluster over `solana-rpc-client` (the cross-repo choice, as quotebook), at `confirmed`
//! commitment.
//!
//! The URL is scrubbed from every error and hidden from `Debug`: RPC providers put API keys in
//! it, and the HTTP client's errors quote the request URL. Ported from metalbook-cli (its
//! final pattern after codex's reviews): the whole message is redacted after the preflight
//! logs are appended, and every path segment or query value of 8 or more characters of the URL
//! is scrubbed as a bare token.

use solana_commitment_config::CommitmentConfig;
use solana_hash::Hash;
use solana_rpc_client::rpc_client::RpcClient;
use solana_rpc_client_api::client_error::{Error as ClientError, ErrorKind};
use solana_rpc_client_api::request::{RpcError, RpcResponseErrorData};
use solana_transaction::Transaction;
use varswap_sdk::Address;

use crate::error::CliError;
use crate::{AccountInfo, Rpc};

/// What replaces a URL in an error.
const HIDDEN: &str = "<rpc url>";

/// A real cluster.
pub struct Cluster {
    client: RpcClient,
    url: String,
}

impl Cluster {
    /// A client for `url` (for example `https://api.devnet.solana.com`). Nothing is sent until
    /// the first call.
    #[must_use]
    pub fn new(url: &str) -> Self {
        Self::with_client(RpcClient::new_with_commitment(
            url.to_owned(),
            CommitmentConfig::confirmed(),
        ))
    }

    /// Wraps an existing client (tests use `RpcClient::new_mock`).
    #[must_use]
    pub fn with_client(client: RpcClient) -> Self {
        let url = client.url();
        Self { client, url }
    }

    fn error(&self, e: &ClientError) -> CliError {
        client_error(e, &self.url)
    }
}

impl std::fmt::Debug for Cluster {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Cluster").field(&HIDDEN).finish()
    }
}

impl Rpc for Cluster {
    fn latest_blockhash(&mut self) -> Result<Hash, CliError> {
        self.client
            .get_latest_blockhash()
            .map_err(|e| self.error(&e))
    }

    fn account(&mut self, address: &Address) -> Result<Option<AccountInfo>, CliError> {
        self.client
            .get_account_with_commitment(address, CommitmentConfig::confirmed())
            .map(|r| {
                r.value.map(|a| AccountInfo {
                    owner: a.owner,
                    data: a.data,
                })
            })
            .map_err(|e| self.error(&e))
    }

    fn send(&mut self, tx: &Transaction) -> Result<(), CliError> {
        self.client
            .send_and_confirm_transaction(tx)
            .map(|_| ())
            .map_err(|e| self.error(&e))
    }
}

/// An RPC client error as a [`CliError::Rpc`]: its message followed by the program logs of a
/// failed preflight simulation, the whole text redacted (a provider may echo its URL or key
/// anywhere, logs included).
#[must_use]
pub fn client_error(e: &ClientError, url: &str) -> CliError {
    let mut message = e.to_string();
    if let ErrorKind::RpcError(RpcError::RpcResponseError {
        data: RpcResponseErrorData::SendTransactionPreflightFailure(simulation),
        ..
    }) = e.kind()
    {
        for line in simulation.logs.iter().flatten() {
            message.push_str("\n  ");
            message.push_str(line);
        }
    }
    CliError::Rpc(redact(&message, url))
}

/// Shortest URL path segment or query value treated as a possible secret.
const MIN_SECRET_LEN: usize = 8;

/// The parts of `url` that can carry a key: path segments and query values of at least
/// [`MIN_SECRET_LEN`] characters (`/?api-key=KEY`, `/TOKEN/`), longest first.
fn secrets_of(url: &str) -> Vec<&str> {
    let after_scheme = url.split_once("://").map_or(url, |(_, rest)| rest);
    // Drop the host: everything up to the first `/` or `?`.
    let tail = after_scheme
        .find(['/', '?'])
        .and_then(|i| after_scheme.split_at_checked(i))
        .map_or("", |(_, tail)| tail);
    let mut parts: Vec<&str> = tail
        .split(['/', '?', '&', '=', '#'])
        .filter(|p| p.len() >= MIN_SECRET_LEN)
        .collect();
    parts.sort_by_key(|p| std::cmp::Reverse(p.len()));
    parts
}

/// `text` with `url`, every other `http://` or `https://` URL, and every path segment or query
/// value of `url` of 8 or more characters replaced by placeholders. The HTTP client may quote a
/// normalized form
/// of the URL (a trailing slash added, the query re-encoded), and a response may echo the key
/// alone, so exact replacement of the URL is not enough.
#[must_use]
pub fn redact(text: &str, url: &str) -> String {
    let mut text = if url.is_empty() {
        text.to_owned()
    } else {
        text.replace(url, HIDDEN)
    };
    for secret in secrets_of(url) {
        text = text.replace(secret, "<redacted>");
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text.as_str();
    while let Some(start) = ["http://", "https://"]
        .iter()
        .filter_map(|scheme| rest.find(scheme))
        .min()
    {
        let Some((before, tail)) = rest.split_at_checked(start) else {
            break;
        };
        out.push_str(before);
        out.push_str(HIDDEN);
        let end = tail
            .find(|c: char| c.is_whitespace() || matches!(c, ')' | '"' | '\'' | '>'))
            .unwrap_or(tail.len());
        rest = tail.split_at_checked(end).map_or("", |(_, after)| after);
    }
    out.push_str(rest);
    out
}

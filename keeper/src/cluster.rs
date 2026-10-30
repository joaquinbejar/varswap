//! [`Transport`] over `solana-rpc-client` (the cross-repo choice, as `varswap-admin`), at
//! `confirmed` commitment.
//!
//! The URL is hidden from `Debug` and scrubbed from every error, with the preflight logs, by the
//! same redactor as the Hermes errors: RPC providers put API keys in the URL and the HTTP
//! client quotes it.

use solana_commitment_config::CommitmentConfig;
use solana_instruction::Instruction;
use solana_keypair::Keypair;
use solana_rpc_client::rpc_client::RpcClient;
use solana_rpc_client_api::client_error::{Error as ClientError, ErrorKind};
use solana_rpc_client_api::request::{RpcError, RpcResponseErrorData};
use solana_signer::Signer;
use solana_transaction::Transaction;
use varswap_sdk::Address;

use crate::receiver_chain::Transport;
use crate::KeeperError;

/// What replaces a URL in an error.
const HIDDEN: &str = "<rpc url>";

/// The Clock sysvar.
const CLOCK: Address = Address::from_str_const("SysvarC1ock11111111111111111111111111111111");

/// Offset of `unix_timestamp` in the Clock sysvar (`slot`, `epoch_start_timestamp`, `epoch`,
/// `leader_schedule_epoch`, then `unix_timestamp`, 8 bytes each).
const CLOCK_UNIX_TIMESTAMP: usize = 32;

/// A real cluster.
pub struct RpcTransport {
    client: RpcClient,
    url: String,
}

impl RpcTransport {
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

    fn error(&self, e: &ClientError) -> KeeperError {
        client_error(e, &self.url)
    }
}

impl core::fmt::Debug for RpcTransport {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_tuple("RpcTransport").field(&HIDDEN).finish()
    }
}

impl Transport for RpcTransport {
    fn now(&self) -> Result<i64, KeeperError> {
        let clock = self
            .client
            .get_account_with_commitment(&CLOCK, CommitmentConfig::confirmed())
            .map_err(|e| self.error(&e))?
            .value
            .ok_or_else(|| KeeperError::Chain("the Clock sysvar is missing".into()))?;
        clock
            .data
            .get(CLOCK_UNIX_TIMESTAMP..)
            .and_then(|d| d.first_chunk::<8>())
            .map(|b| i64::from_le_bytes(*b))
            .ok_or_else(|| KeeperError::Chain("the Clock sysvar is malformed".into()))
    }

    fn account(&self, account: &Address) -> Result<Option<(Address, Vec<u8>)>, KeeperError> {
        self.client
            .get_account_with_commitment(account, CommitmentConfig::confirmed())
            .map(|r| r.value.map(|a| (a.owner, a.data)))
            .map_err(|e| self.error(&e))
    }

    fn rent_exempt(&self, len: usize) -> Result<u64, KeeperError> {
        self.client
            .get_minimum_balance_for_rent_exemption(len)
            .map_err(|e| self.error(&e))
    }

    fn send(
        &mut self,
        instructions: &[Instruction],
        payer: &Keypair,
        extra: Option<&Keypair>,
    ) -> Result<String, KeeperError> {
        let blockhash = self
            .client
            .get_latest_blockhash()
            .map_err(|e| self.error(&e))?;
        let mut tx = Transaction::new_with_payer(instructions, Some(&payer.pubkey()));
        let signed = match extra {
            Some(k) => tx.try_sign(&[payer, k], blockhash),
            None => tx.try_sign(&[payer], blockhash),
        };
        signed.map_err(|_| KeeperError::Chain("the transaction needs another signer".into()))?;
        let signature = tx
            .signatures
            .first()
            .map(ToString::to_string)
            .ok_or_else(|| KeeperError::Chain("no signature".into()))?;
        self.client
            .send_and_confirm_transaction(&tx)
            .map_err(|e| self.error(&e))?;
        Ok(signature)
    }
}

/// An RPC client error as a [`KeeperError::Chain`]: its message with the program logs of a
/// failed preflight simulation, the whole text redacted. `solana-rpc-client` 4.3 already
/// renders the logs in the message; a line is appended only if the message lacks it.
#[must_use]
pub fn client_error(e: &ClientError, url: &str) -> KeeperError {
    let mut message = e.to_string().trim_end().to_owned();
    if let ErrorKind::RpcError(RpcError::RpcResponseError {
        data: RpcResponseErrorData::SendTransactionPreflightFailure(simulation),
        ..
    }) = e.kind()
    {
        for line in simulation.logs.iter().flatten() {
            if !message.contains(line.as_str()) {
                message.push_str("\n  ");
                message.push_str(line);
            }
        }
    }
    KeeperError::Chain(crate::redact::scrub(&message, url, "", HIDDEN))
}

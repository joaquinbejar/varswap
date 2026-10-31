//! Posting a Hermes update as a `Full` `PriceUpdateV2` and using it in the same transaction,
//! over any [`Transport`] (an RPC client on a cluster, LiteSVM in tests).
//!
//! One post is a fixed sequence of transactions ([`post_plan`]):
//!
//! 1. create the encoded VAA account (owned by Wormhole, `46 + vaa_len` bytes),
//!    `init_encoded_vaa`, and write the first [`FIRST_CHUNK`] bytes of the VAA;
//! 2. write the rest in chunks of at most [`NEXT_CHUNK`] bytes; the last write carries a compute
//!    unit limit and `verify_encoded_vaa_v1` (one `secp256k1_recover` per signature);
//! 3. `post_update` (a fresh `PriceUpdateV2` account, `Full` because the VAA is verified), the
//!    caller's instructions that read it, then `reclaim_rent` and `close_encoded_vaa`, so a post
//!    leaves no account behind and returns all its rent.
//!
//! Every transaction fits the 1,232-byte packet limit for a VAA of 19 signatures (tested). When a
//! send fails, [`post`] re-reads the encoded VAA and closes it if the chain holds it, whatever
//! the failed send reported (its transaction may have landed with the confirmation lost), so a
//! failed post costs fees only.

use solana_address::Address;
use solana_instruction::Instruction;
use solana_keypair::Keypair;
use solana_signer::Signer;

use crate::accumulator::{AccumulatorUpdate, MerkleUpdate};
use crate::ix::{self, ENCODED_VAA_HEADER, WORMHOLE_PROGRAM};
use crate::PostError;

/// VAA bytes written by the first transaction, next to the account creation.
pub const FIRST_CHUNK: usize = 800;

/// VAA bytes written by each later transaction.
pub const NEXT_CHUNK: usize = 850;

/// Compute unit limit of the transaction that verifies the VAA: 19 signatures at 25,000 units
/// each for `secp256k1_recover`, plus the write and the parsing.
pub const VERIFY_COMPUTE_UNITS: u32 = 1_000_000;

/// How a poster reaches a cluster.
pub trait Transport {
    /// Current cluster time (Clock sysvar), seconds.
    ///
    /// # Errors
    ///
    /// `PostError::Chain` if the clock cannot be read.
    fn now(&self) -> Result<i64, PostError>;

    /// The owner and data of `account`, or `None` if it does not exist.
    ///
    /// # Errors
    ///
    /// `PostError::Chain` if the account cannot be read.
    fn account(&self, account: &Address) -> Result<Option<(Address, Vec<u8>)>, PostError>;

    /// The current block height (`confirmed`).
    ///
    /// # Errors
    ///
    /// `PostError::Chain` if the cluster cannot answer.
    fn block_height(&self) -> Result<u64, PostError>;

    /// The last block height at which the transaction of the most recent [`Transport::send`]
    /// can land, whatever that call returned: its blockhash's `last_valid_block_height`. `None`
    /// before any send, or when the transport cannot tell.
    fn last_send_expiry(&self) -> Option<u64>;

    /// The accounts owned by `program` whose data holds `bytes` at `offset`
    /// (`getProgramAccounts` with one `memcmp` filter).
    ///
    /// # Errors
    ///
    /// `PostError::Chain` if the cluster cannot answer.
    fn program_accounts(
        &self,
        program: &Address,
        offset: usize,
        bytes: &[u8],
    ) -> Result<Vec<Address>, PostError>;

    /// Lamports that make an account of `len` bytes rent exempt.
    ///
    /// # Errors
    ///
    /// `PostError::Chain` if the cluster cannot answer.
    fn rent_exempt(&self, len: usize) -> Result<u64, PostError>;

    /// Signs `instructions` with `payer` (fee payer) and `extra`, sends them as one transaction
    /// and waits for confirmation. Returns the signature.
    ///
    /// # Errors
    ///
    /// `PostError::Chain` if the transaction is rejected, fails or is not confirmed.
    fn send(
        &mut self,
        instructions: &[Instruction],
        payer: &Keypair,
        extra: Option<&Keypair>,
    ) -> Result<String, PostError>;
}

/// Where a poster gets Hermes' binary update.
///
/// Implemented for references too, so one source (one cache, one rate limit) can serve both
/// the price queries and the posts.
pub trait UpdateSource {
    /// The binary accumulator update whose message for `feed` was published at
    /// `publish_time` (Hermes `GET /v2/updates/price/{publish_time}`, the first update at or
    /// after it).
    ///
    /// # Errors
    ///
    /// `PostError::Source` if the source cannot answer or has no such update,
    /// `PostError::RateLimited` on a 429.
    fn update(&self, feed: &[u8; 32], publish_time: i64) -> Result<Vec<u8>, PostError>;
}

impl<U: UpdateSource + ?Sized> UpdateSource for &U {
    fn update(&self, feed: &[u8; 32], publish_time: i64) -> Result<Vec<u8>, PostError> {
        (**self).update(feed, publish_time)
    }
}

/// Which fresh keypair, besides the payer, a transaction of a post needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Extra {
    /// Only the payer signs.
    None,
    /// The encoded VAA account signs its creation.
    EncodedVaa,
    /// The `PriceUpdateV2` account signs its creation.
    PriceUpdate,
}

/// One transaction of a post.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    /// Its instructions, in order.
    pub instructions: Vec<Instruction>,
    /// The fresh keypair that signs it besides the payer.
    pub extra: Extra,
}

/// The accounts and amounts of one post.
#[derive(Debug, Clone, Copy)]
pub struct PostAccounts<'a> {
    /// Fee payer, rent payer and write authority of both fresh accounts.
    pub payer: &'a Address,
    /// The fresh encoded VAA account.
    pub encoded_vaa: &'a Address,
    /// The fresh `PriceUpdateV2` account.
    pub price_update: &'a Address,
    /// Rent of the encoded VAA account (returned by `close_encoded_vaa`).
    pub encoded_vaa_rent: u64,
    /// The receiver treasury id (`0`).
    pub treasury_id: u8,
}

/// The transactions that post `update` (one message of `accumulator`), run `consume` (the
/// caller's instructions that read the `PriceUpdateV2`) and close both accounts.
///
/// # Errors
///
/// `PostError::Update` if the VAA is not version 1, or a length does not fit its field.
pub fn post_plan(
    a: &PostAccounts<'_>,
    accumulator: &AccumulatorUpdate,
    update: &MerkleUpdate,
    consume: Vec<Instruction>,
) -> Result<Vec<Step>, PostError> {
    let vaa = accumulator.vaa.as_slice();
    let index = accumulator.guardian_set_index()?;
    let space = vaa
        .len()
        .checked_add(ENCODED_VAA_HEADER)
        .and_then(|n| u64::try_from(n).ok())
        .ok_or(PostError::Update("VAA too long"))?;
    let write = |offset: usize, chunk: &[u8]| {
        u32::try_from(offset)
            .ok()
            .and_then(|o| ix::write_encoded_vaa(a.payer, a.encoded_vaa, o, chunk))
            .ok_or(PostError::Update("VAA too long"))
    };

    let (first, mut rest) = vaa.split_at_checked(FIRST_CHUNK).unwrap_or((vaa, &[]));
    let mut steps = vec![Step {
        instructions: vec![
            ix::create_account(
                a.payer,
                a.encoded_vaa,
                a.encoded_vaa_rent,
                space,
                &WORMHOLE_PROGRAM,
            ),
            ix::init_encoded_vaa(a.payer, a.encoded_vaa),
            write(0, first)?,
        ],
        extra: Extra::EncodedVaa,
    }];
    // Every later chunk but the last gets its own transaction; the last one goes with the
    // verification.
    let mut offset = first.len();
    let mut pending: Option<Instruction> = None;
    while !rest.is_empty() {
        let (chunk, tail) = rest.split_at_checked(NEXT_CHUNK).unwrap_or((rest, &[]));
        if let Some(previous) = pending.replace(write(offset, chunk)?) {
            steps.push(Step {
                instructions: vec![previous],
                extra: Extra::None,
            });
        }
        offset = offset
            .checked_add(chunk.len())
            .ok_or(PostError::Update("VAA too long"))?;
        rest = tail;
    }
    let mut verify = vec![ix::compute_unit_limit(VERIFY_COMPUTE_UNITS)];
    verify.extend(pending);
    verify.push(ix::verify_encoded_vaa_v1(a.payer, a.encoded_vaa, index));
    steps.push(Step {
        instructions: verify,
        extra: Extra::None,
    });

    let post = ix::post_update(
        a.payer,
        a.encoded_vaa,
        a.price_update,
        update,
        a.treasury_id,
    )
    .ok_or(PostError::Update("price message too long"))?;
    let mut last = vec![post];
    last.extend(consume);
    last.push(ix::reclaim_rent(a.payer, a.price_update));
    last.push(ix::close_encoded_vaa(a.payer, a.encoded_vaa));
    steps.push(Step {
        instructions: last,
        extra: Extra::PriceUpdate,
    });
    Ok(steps)
}

/// Posts `update` (one message of `accumulator`) through Wormhole and the Pyth receiver,
/// runs the instructions `consume` builds for the fresh `PriceUpdateV2` address, and closes
/// both accounts. Returns the signatures, in order.
///
/// # Errors
///
/// `PostError::Update` for a VAA that cannot be planned, `PostError::Chain` from the
/// transport. After any failed send the encoded VAA is re-read and closed if it exists (a
/// transaction whose confirmation was lost may have landed); if it is still open after two
/// closes, the error is [`PostError::Orphaned`] with its address; if the first transaction
/// failed and the account does not exist, it is [`PostError::Uncertain`] with the last block
/// height at which that transaction can still land.
pub fn post<T: Transport>(
    transport: &mut T,
    payer: &Keypair,
    accumulator: &AccumulatorUpdate,
    update: &MerkleUpdate,
    treasury_id: u8,
    consume: impl FnOnce(&Address) -> Vec<Instruction>,
) -> Result<Vec<String>, PostError> {
    let vaa = Keypair::new();
    let pu = Keypair::new();
    let space = accumulator
        .vaa
        .len()
        .checked_add(ENCODED_VAA_HEADER)
        .ok_or(PostError::Update("VAA too long"))?;
    let rent = transport.rent_exempt(space)?;
    let steps = post_plan(
        &PostAccounts {
            payer: &payer.pubkey(),
            encoded_vaa: &vaa.pubkey(),
            price_update: &pu.pubkey(),
            encoded_vaa_rent: rent,
            treasury_id,
        },
        accumulator,
        update,
        consume(&pu.pubkey()),
    )?;
    let mut signatures = Vec::with_capacity(steps.len());
    for (i, step) in steps.iter().enumerate() {
        let extra = match step.extra {
            Extra::None => None,
            Extra::EncodedVaa => Some(&vaa),
            Extra::PriceUpdate => Some(&pu),
        };
        match transport.send(&step.instructions, payer, extra) {
            Ok(signature) => signatures.push(signature),
            Err(e) => {
                // The failed transaction may still have landed (a lost confirmation), so the
                // chain, not this loop, says whether the encoded VAA exists.
                let expiry = transport.last_send_expiry();
                let existed = matches!(
                    transport.account(&vaa.pubkey()),
                    Ok(Some((owner, _))) if owner == WORMHOLE_PROGRAM
                );
                return Err(match close_if_open(transport, payer, &vaa.pubkey()) {
                    Some(encoded_vaa) => PostError::Orphaned {
                        encoded_vaa,
                        cause: e.to_string(),
                    },
                    // Only the first transaction creates the account: if that one failed and
                    // the account was not there, it may still be in flight.
                    None => match (i, expiry) {
                        (0, Some(valid_until)) if !existed => PostError::Uncertain {
                            encoded_vaa: vaa.pubkey(),
                            valid_until,
                            cause: e.to_string(),
                        },
                        _ => e,
                    },
                });
            }
        }
    }
    Ok(signatures)
}

/// Offset of the write authority in an encoded VAA account (after the 8-byte discriminator and
/// the 1-byte status).
const ENCODED_VAA_WRITE_AUTHORITY: usize = 9;

/// Every encoded VAA account on the cluster written by `write_authority`, including any a post
/// has in flight right now (in this process or in another one using the same payer). Use
/// [`LeftoverScanner`] to tell leftovers from posts in progress.
///
/// # Errors
///
/// `PostError::Chain` if the cluster cannot answer.
pub fn scan_encoded_vaas<T: Transport>(
    transport: &T,
    write_authority: &Address,
) -> Result<Vec<Address>, PostError> {
    transport.program_accounts(
        &WORMHOLE_PROGRAM,
        ENCODED_VAA_WRITE_AUTHORITY,
        write_authority.as_ref(),
    )
}

/// Blocks an encoded VAA must stay unchanged on the cluster, as seen by consecutive scans,
/// before [`LeftoverScanner`] reports it: two blockhash lifetimes (150 blocks each). A post
/// creates, writes, verifies and closes its encoded VAA within seconds, changing it at every
/// step, so one that has not changed for 300 blocks is a leftover, not a post in flight.
pub const LEFTOVER_MIN_AGE: u64 = 300;

/// Finds leftover encoded VAAs (a post gave up on them and they stayed open) without touching
/// a post in flight. Each [`LeftoverScanner::scan`] lists the payer's encoded VAAs
/// ([`scan_encoded_vaas`]), reads each one, then reads the block height, and reports only
/// those whose content (status and written bytes) every scan has seen unchanged since a height
/// at least [`LEFTOVER_MIN_AGE`] blocks ago. Any change (a write, `Writing` to `Verified`)
/// starts the count over, and so does a scan that misses the account; a first sighting is
/// never old, because the height is read after the listing. A post in flight, here or in
/// another process with the same payer, keeps changing its encoded VAA until it closes it, so
/// it is never reported unless it stalls for 300 blocks; closing what is reported cannot break
/// a live post (the receiver's `WrongVaaOwner`, 6013, on a VAA closed mid-post).
///
/// Run one poster process per payer anyway: two processes sharing a payer also share its fees
/// and every other account it writes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LeftoverScanner {
    /// Each encoded VAA seen by the last scan: its content then, and the block height since
    /// which every scan has seen exactly that content.
    seen: Vec<(Address, Vec<u8>, u64)>,
}

impl LeftoverScanner {
    /// A scanner that has seen nothing yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The encoded VAAs written by `write_authority` whose content has been unchanged for at
    /// least [`LEFTOVER_MIN_AGE`] blocks across consecutive scans.
    ///
    /// # Errors
    ///
    /// `PostError::Chain` if the cluster cannot answer (the scanner's state is unchanged).
    pub fn scan<T: Transport>(
        &mut self,
        transport: &T,
        write_authority: &Address,
    ) -> Result<Vec<Address>, PostError> {
        let mut current = Vec::new();
        for address in scan_encoded_vaas(transport, write_authority)? {
            if let Some((owner, data)) = transport.account(&address)? {
                if owner == WORMHOLE_PROGRAM {
                    current.push((address, data));
                }
            }
        }
        // After the listing and the reads, however long they took: a first sighting gets age
        // zero, never a height from before it existed.
        let height = transport.block_height()?;
        let seen: Vec<(Address, Vec<u8>, u64)> = current
            .into_iter()
            .map(|(address, data)| {
                let since = self
                    .seen
                    .iter()
                    .find(|(a, d, _)| *a == address && *d == data)
                    .map_or(height, |(_, _, since)| *since);
                (address, data, since)
            })
            .collect();
        let old = seen
            .iter()
            .filter(|(_, _, since)| {
                since
                    .checked_add(LEFTOVER_MIN_AGE)
                    .is_some_and(|due| height >= due)
            })
            .map(|(address, _, _)| *address)
            .collect();
        self.seen = seen;
        Ok(old)
    }
}

/// How many times [`close_if_open`] sends `close_encoded_vaa` before giving up.
pub const CLOSE_ATTEMPTS: usize = 2;

/// Closes the encoded VAA at `address` (written by `payer`) if the chain still holds it (owned
/// by Wormhole), re-reading after every attempt: a close whose confirmation was lost but that
/// landed is seen as closed, so it is never retried. Returns the address if it is still open
/// (or cannot be read) after [`CLOSE_ATTEMPTS`] closes. [`post`] calls it after a failed send;
/// callers retry a [`PostError::Orphaned`] address with it.
#[must_use]
pub fn close_if_open<T: Transport>(
    transport: &mut T,
    payer: &Keypair,
    address: &Address,
) -> Option<Address> {
    let open = |t: &T| match t.account(address) {
        Ok(Some((owner, _))) => owner == WORMHOLE_PROGRAM,
        Ok(None) => false,
        // Unknown: treat as open, so the caller hears about it.
        Err(_) => true,
    };
    for _ in 0..CLOSE_ATTEMPTS {
        if !open(transport) {
            return None;
        }
        let close = [ix::close_encoded_vaa(&payer.pubkey(), address)];
        // The result is not trusted either way; the next read decides.
        let _ = transport.send(&close, payer, None);
    }
    open(transport).then_some(*address)
}

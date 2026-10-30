//! The production [`Chain`]: posts each canonical print through Wormhole and the Pyth receiver,
//! then calls `sample`, over any [`Transport`] (an RPC client on a cluster, LiteSVM in tests).
//!
//! One post is a fixed sequence of transactions ([`post_plan`]):
//!
//! 1. create the encoded VAA account (owned by Wormhole, `46 + vaa_len` bytes),
//!    `init_encoded_vaa`, and write the first [`FIRST_CHUNK`] bytes of the VAA;
//! 2. write the rest in chunks of at most [`NEXT_CHUNK`] bytes; the last write carries a compute
//!    unit limit and `verify_encoded_vaa_v1` (one `secp256k1_recover` per signature);
//! 3. `post_update` (a fresh `PriceUpdateV2` account, `Full` because the VAA is verified),
//!    varswap `sample` on it, then `reclaim_rent` and `close_encoded_vaa`, so a post leaves no
//!    account behind and returns all its rent.
//!
//! Every transaction fits the 1,232-byte packet limit for a VAA of 19 signatures (tested). If a
//! step fails after the encoded VAA exists, the chain tries to close it before returning the
//! error, so a failed post costs fees only.

use solana_instruction::Instruction;
use solana_keypair::Keypair;
use solana_signer::Signer;
use varswap_sdk::{ix, Address};

use crate::accumulator::{AccumulatorUpdate, MerkleUpdate};
use crate::receiver::{self, ENCODED_VAA_HEADER, WORMHOLE_PROGRAM};
use crate::{Chain, KeeperError, PrintMsg};

/// VAA bytes written by the first transaction, next to the account creation.
pub const FIRST_CHUNK: usize = 800;

/// VAA bytes written by each later transaction.
pub const NEXT_CHUNK: usize = 850;

/// Compute unit limit of the transaction that verifies the VAA: 19 signatures at 25,000 units
/// each for `secp256k1_recover`, plus the write and the parsing.
pub const VERIFY_COMPUTE_UNITS: u32 = 1_000_000;

/// How the keeper reaches a cluster.
pub trait Transport {
    /// Current cluster time (Clock sysvar), seconds.
    ///
    /// # Errors
    ///
    /// `KeeperError::Chain` if the clock cannot be read.
    fn now(&self) -> Result<i64, KeeperError>;

    /// The owner and data of `account`, or `None` if it does not exist.
    ///
    /// # Errors
    ///
    /// `KeeperError::Chain` if the account cannot be read.
    fn account(&self, account: &Address) -> Result<Option<(Address, Vec<u8>)>, KeeperError>;

    /// Lamports that make an account of `len` bytes rent exempt.
    ///
    /// # Errors
    ///
    /// `KeeperError::Chain` if the cluster cannot answer.
    fn rent_exempt(&self, len: usize) -> Result<u64, KeeperError>;

    /// Signs `instructions` with `payer` (fee payer) and `extra`, sends them as one transaction
    /// and waits for confirmation. Returns the signature.
    ///
    /// # Errors
    ///
    /// `KeeperError::Chain` if the transaction is rejected, fails or is not confirmed.
    fn send(
        &mut self,
        instructions: &[Instruction],
        payer: &Keypair,
        extra: Option<&Keypair>,
    ) -> Result<String, KeeperError>;
}

/// Where the chain gets the binary Hermes update that proves a print.
pub trait UpdateSource {
    /// Hermes' binary accumulator update containing `print` for `feed`.
    ///
    /// # Errors
    ///
    /// `KeeperError::Source` if the source cannot answer or has no update for that print.
    fn update(&self, feed: &[u8; 32], print: &PrintMsg) -> Result<Vec<u8>, KeeperError>;
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
    /// The varswap program.
    pub program_id: &'a Address,
    /// The window to sample.
    pub window: &'a Address,
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

/// The transactions that post `update` (one message of `accumulator`) and sample it.
///
/// # Errors
///
/// `KeeperError::Update` if the VAA is not version 1, or a length does not fit its field.
pub fn post_plan(
    a: &PostAccounts<'_>,
    accumulator: &AccumulatorUpdate,
    update: &MerkleUpdate,
) -> Result<Vec<Step>, KeeperError> {
    let vaa = accumulator.vaa.as_slice();
    let index = accumulator.guardian_set_index()?;
    let space = vaa
        .len()
        .checked_add(ENCODED_VAA_HEADER)
        .and_then(|n| u64::try_from(n).ok())
        .ok_or(KeeperError::Update("VAA too long"))?;
    let (first, mut rest) = vaa.split_at_checked(FIRST_CHUNK).unwrap_or((vaa, &[]));
    let write = |offset: usize, chunk: &[u8]| {
        u32::try_from(offset)
            .ok()
            .and_then(|o| receiver::write_encoded_vaa(a.payer, a.encoded_vaa, o, chunk))
            .ok_or(KeeperError::Update("VAA too long"))
    };

    let mut steps = vec![Step {
        instructions: vec![
            receiver::create_account(
                a.payer,
                a.encoded_vaa,
                a.encoded_vaa_rent,
                space,
                &WORMHOLE_PROGRAM,
            ),
            receiver::init_encoded_vaa(a.payer, a.encoded_vaa),
            write(0, first)?,
        ],
        extra: Extra::EncodedVaa,
    }];
    let mut offset = first.len();
    let mut last = Vec::new();
    while !rest.is_empty() {
        let (chunk, tail) = rest.split_at_checked(NEXT_CHUNK).unwrap_or((rest, &[]));
        if !last.is_empty() {
            steps.push(Step {
                instructions: core::mem::take(&mut last),
                extra: Extra::None,
            });
        }
        last.push(write(offset, chunk)?);
        offset = offset
            .checked_add(chunk.len())
            .ok_or(KeeperError::Update("VAA too long"))?;
        rest = tail;
    }
    let mut verify = vec![receiver::compute_unit_limit(VERIFY_COMPUTE_UNITS)];
    verify.append(&mut last);
    verify.push(receiver::verify_encoded_vaa_v1(
        a.payer,
        a.encoded_vaa,
        index,
    ));
    steps.push(Step {
        instructions: verify,
        extra: Extra::None,
    });

    let post = receiver::post_update(
        a.payer,
        a.encoded_vaa,
        a.price_update,
        update,
        a.treasury_id,
    )
    .ok_or(KeeperError::Update("price message too long"))?;
    steps.push(Step {
        instructions: vec![
            post,
            ix::sample(a.program_id, a.window, a.price_update),
            receiver::reclaim_rent(a.payer, a.price_update),
            receiver::close_encoded_vaa(a.payer, a.encoded_vaa),
        ],
        extra: Extra::PriceUpdate,
    });
    Ok(steps)
}

/// The keeper's chain on a real cluster: prints go through Wormhole and the Pyth receiver.
pub struct ReceiverChain<T: Transport, U: UpdateSource> {
    transport: T,
    updates: U,
    payer: Keypair,
    program_id: Address,
    treasury_id: u8,
    signatures: Vec<String>,
}

impl<T: Transport, U: UpdateSource> ReceiverChain<T, U> {
    /// A chain for the varswap program `program_id`, paying with `payer`.
    #[must_use]
    pub fn new(transport: T, updates: U, payer: Keypair, program_id: Address) -> Self {
        Self {
            transport,
            updates,
            payer,
            program_id,
            treasury_id: 0,
            signatures: Vec::new(),
        }
    }

    /// The signatures of every transaction this chain sent, in order.
    #[must_use]
    pub fn signatures(&self) -> &[String] {
        &self.signatures
    }

    /// The transport, for reads.
    #[must_use]
    pub fn transport(&self) -> &T {
        &self.transport
    }

    fn send(&mut self, step: &Step, vaa: &Keypair, pu: &Keypair) -> Result<(), KeeperError> {
        let extra = match step.extra {
            Extra::None => None,
            Extra::EncodedVaa => Some(vaa),
            Extra::PriceUpdate => Some(pu),
        };
        let signature = self
            .transport
            .send(&step.instructions, &self.payer, extra)?;
        self.signatures.push(signature);
        Ok(())
    }
}

impl<T: Transport, U: UpdateSource> core::fmt::Debug for ReceiverChain<T, U> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ReceiverChain")
            .field("payer", &self.payer.pubkey())
            .field("program_id", &self.program_id)
            .finish_non_exhaustive()
    }
}

impl<T: Transport, U: UpdateSource> Chain for ReceiverChain<T, U> {
    fn now(&self) -> Result<i64, KeeperError> {
        self.transport.now()
    }

    fn account(&self, account: &Address) -> Result<Option<(Address, Vec<u8>)>, KeeperError> {
        self.transport.account(account)
    }

    fn post_sample(
        &mut self,
        window: &Address,
        feed: &[u8; 32],
        print: &PrintMsg,
    ) -> Result<(), KeeperError> {
        let bytes = self.updates.update(feed, print)?;
        let accumulator = AccumulatorUpdate::parse(&bytes)?;
        let (update, decoded) = accumulator.find(feed).ok_or(KeeperError::Update(
            "the update has no message for the feed",
        ))?;
        if decoded != *print {
            return Err(KeeperError::Update(
                "the update's message is not the print to post",
            ));
        }
        let vaa = Keypair::new();
        let pu = Keypair::new();
        let space = accumulator
            .vaa
            .len()
            .checked_add(ENCODED_VAA_HEADER)
            .ok_or(KeeperError::Update("VAA too long"))?;
        let rent = self.transport.rent_exempt(space)?;
        let steps = post_plan(
            &PostAccounts {
                program_id: &self.program_id,
                window,
                payer: &self.payer.pubkey(),
                encoded_vaa: &vaa.pubkey(),
                price_update: &pu.pubkey(),
                encoded_vaa_rent: rent,
                treasury_id: self.treasury_id,
            },
            &accumulator,
            update,
        )?;
        let mut created = false;
        for step in &steps {
            if let Err(e) = self.send(step, &vaa, &pu) {
                if created {
                    // Best effort: return the encoded VAA's rent; the post's error is the one
                    // that matters.
                    let close = Step {
                        instructions: vec![receiver::close_encoded_vaa(
                            &self.payer.pubkey(),
                            &vaa.pubkey(),
                        )],
                        extra: Extra::None,
                    };
                    let _ = self.send(&close, &vaa, &pu);
                }
                return Err(e);
            }
            created = true;
        }
        Ok(())
    }

    fn finalize(&mut self, window: &Address) -> Result<(), KeeperError> {
        let step = Step {
            instructions: vec![ix::finalize(&self.program_id, window)],
            extra: Extra::None,
        };
        let unused = Keypair::new();
        self.send(&step, &unused, &unused)
    }
}

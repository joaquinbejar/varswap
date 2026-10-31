//! The production [`Chain`]: each canonical print goes through Wormhole and the Pyth receiver as
//! a `Full` `PriceUpdateV2` and is sampled in the transaction that posts it
//! ([`pyth_post::post`]), over any [`Transport`] (an RPC client on a cluster, LiteSVM in tests).

use pyth_post::{
    close_if_open, post, AccumulatorUpdate, LeftoverScanner, PostError, Transport, UpdateSource,
};
use solana_instruction::Instruction;
use solana_keypair::Keypair;
use solana_signer::Signer;
use varswap_sdk::{ix, Address};

use crate::{Chain, KeeperError, PrintMsg};

/// An encoded VAA a failed post may have left open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Orphan {
    /// The encoded VAA account.
    pub encoded_vaa: Address,
    /// `None`: it exists (or could not be closed). `Some(h)`: it did not exist yet, but the
    /// transaction that creates it can land until block height `h`; it is kept until then.
    pub valid_until: Option<u64>,
}

/// The keeper's chain on a real cluster: prints go through Wormhole and the Pyth receiver.
pub struct ReceiverChain<T: Transport, U: UpdateSource> {
    transport: T,
    updates: U,
    payer: Keypair,
    program_id: Address,
    treasury_id: u8,
    signatures: Vec<String>,
    /// Encoded VAAs that failed posts left open or may still open, retried before every
    /// later send.
    orphans: Vec<Orphan>,
    /// Ages the payer's encoded VAAs, so a scan adopts only leftovers.
    scanner: LeftoverScanner,
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
            orphans: Vec::new(),
            scanner: LeftoverScanner::new(),
        }
    }

    /// The signatures of every transaction this chain sent, in order.
    #[must_use]
    pub fn signatures(&self) -> &[String] {
        &self.signatures
    }

    /// The transport.
    #[must_use]
    pub fn transport(&self) -> &T {
        &self.transport
    }

    /// The transport, mutably (tests move the clock).
    pub fn transport_mut(&mut self) -> &mut T {
        &mut self.transport
    }

    /// Encoded VAAs that failed posts left open, or whose creating transaction can still
    /// land. The chain retries closing them before every later post or finalize.
    #[must_use]
    pub fn orphans(&self) -> &[Orphan] {
        &self.orphans
    }

    /// Assumes the payer is exclusive to this process (pyth-post's contract; the
    /// `varswap-keeper` binary holds a `pyth_post::PayerLock`).
    ///
    /// Adds to the orphans every leftover encoded VAA written by the payer: one that
    /// consecutive scans have seen for at least `pyth_post::LEFTOVER_MIN_AGE` blocks
    /// ([`LeftoverScanner`]), for example a first transaction that landed after its post gave
    /// up on it. A younger one may be a post in flight, here or in another process with the
    /// same payer, and is left alone. Returns how many were new.
    ///
    /// # Errors
    ///
    /// `KeeperError::Chain` if the cluster cannot answer.
    pub fn scan_orphans(&mut self) -> Result<usize, KeeperError> {
        let found = self.scanner.scan(&self.transport, &self.payer.pubkey())?;
        let mut new = Vec::new();
        for encoded_vaa in found {
            match self
                .orphans
                .iter_mut()
                .find(|o| o.encoded_vaa == encoded_vaa)
            {
                // It exists now: no longer uncertain.
                Some(o) => o.valid_until = None,
                None => new.push(Orphan {
                    encoded_vaa,
                    valid_until: None,
                }),
            }
        }
        let added = new.len();
        self.orphans.extend(new);
        Ok(added)
    }

    /// Retries closing every orphan. One that existed and is now closed is dropped; one that is
    /// absent is dropped too, unless its creating transaction can still land (the block height
    /// is at most its `valid_until`, or cannot be read).
    fn close_orphans(&mut self) {
        let height = self.transport.block_height().ok();
        let (transport, payer) = (&mut self.transport, &self.payer);
        self.orphans.retain(|o| {
            let existed = matches!(
                transport.account(&o.encoded_vaa),
                Ok(Some((owner, _))) if owner == pyth_post::ix::WORMHOLE_PROGRAM
            );
            if close_if_open(transport, payer, &o.encoded_vaa).is_some() {
                return true;
            }
            if existed {
                // It was there and is now closed: whatever was in flight has landed.
                return false;
            }
            match (o.valid_until, height) {
                (Some(until), Some(h)) => h <= until,
                (Some(_), None) => true,
                (None, _) => false,
            }
        });
    }

    fn send(&mut self, instructions: &[Instruction]) -> Result<(), KeeperError> {
        self.close_orphans();
        let signature = self.transport.send(instructions, &self.payer, None)?;
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
        Ok(self.transport.now()?)
    }

    fn account(&self, account: &Address) -> Result<Option<(Address, Vec<u8>)>, KeeperError> {
        Ok(self.transport.account(account)?)
    }

    /// Fetches the update of `print`, checks that its message for `feed` is exactly `print`,
    /// then posts it and calls `sample` on `window` in the posting transaction.
    fn post_sample(
        &mut self,
        window: &Address,
        feed: &[u8; 32],
        print: &PrintMsg,
    ) -> Result<(), KeeperError> {
        let bytes = self.updates.update(feed, print.publish_time)?;
        let accumulator = AccumulatorUpdate::parse(&bytes)?;
        let (update, m) = accumulator.find(feed).ok_or(KeeperError::Update(
            "the update has no message for the feed",
        ))?;
        let decoded = PrintMsg {
            price: m.price,
            conf: m.conf,
            expo: m.exponent,
            publish_time: m.publish_time,
            prev_publish_time: m.prev_publish_time,
        };
        if decoded != *print {
            return Err(KeeperError::Update(
                "the update's message is not the print to post",
            ));
        }
        self.close_orphans();
        let (program_id, window) = (self.program_id, *window);
        let posted = post(
            &mut self.transport,
            &self.payer,
            &accumulator,
            update,
            self.treasury_id,
            |price_update| vec![ix::sample(&program_id, &window, price_update)],
        );
        match posted {
            Ok(signatures) => {
                self.signatures.extend(signatures);
                Ok(())
            }
            Err(e) => {
                match &e {
                    PostError::Orphaned { encoded_vaa, .. } => self.orphans.push(Orphan {
                        encoded_vaa: *encoded_vaa,
                        valid_until: None,
                    }),
                    PostError::Uncertain {
                        encoded_vaa,
                        valid_until,
                        ..
                    } => self.orphans.push(Orphan {
                        encoded_vaa: *encoded_vaa,
                        valid_until: Some(*valid_until),
                    }),
                    _ => {}
                }
                Err(e.into())
            }
        }
    }

    fn finalize(&mut self, window: &Address) -> Result<(), KeeperError> {
        self.send(&[ix::finalize(&self.program_id, window)])
    }
}

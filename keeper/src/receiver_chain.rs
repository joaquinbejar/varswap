//! The production [`Chain`]: each canonical print goes through Wormhole and the Pyth receiver as
//! a `Full` `PriceUpdateV2` and is sampled in the transaction that posts it
//! ([`pyth_post::post`]), over any [`Transport`] (an RPC client on a cluster, LiteSVM in tests).

use pyth_post::{close_if_open, post, AccumulatorUpdate, PostError, Transport, UpdateSource};
use solana_instruction::Instruction;
use solana_keypair::Keypair;
use solana_signer::Signer;
use varswap_sdk::{ix, Address};

use crate::{Chain, KeeperError, PrintMsg};

/// The keeper's chain on a real cluster: prints go through Wormhole and the Pyth receiver.
pub struct ReceiverChain<T: Transport, U: UpdateSource> {
    transport: T,
    updates: U,
    payer: Keypair,
    program_id: Address,
    treasury_id: u8,
    signatures: Vec<String>,
    /// Encoded VAAs that failed posts left open, retried before every later send.
    orphans: Vec<Address>,
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
        }
    }

    /// Encoded VAAs still open after failed posts (each holds its rent). The chain retries
    /// closing them before every later post or finalize.
    #[must_use]
    pub fn orphans(&self) -> &[Address] {
        &self.orphans
    }

    /// Retries closing every orphaned encoded VAA; keeps those still open.
    fn close_orphans(&mut self) {
        let (transport, payer) = (&mut self.transport, &self.payer);
        self.orphans
            .retain(|a| close_if_open(transport, payer, a).is_some());
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
                if let PostError::Orphaned { encoded_vaa, .. } = &e {
                    self.orphans.push(*encoded_vaa);
                }
                Err(e.into())
            }
        }
    }

    fn finalize(&mut self, window: &Address) -> Result<(), KeeperError> {
        self.send(&[ix::finalize(&self.program_id, window)])
    }
}

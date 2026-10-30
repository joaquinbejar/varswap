//! Hermes' binary update: the Pythnet accumulator update (`PNAU`) that the Pyth receiver posts.
//!
//! Wire format (pythnet-sdk 3.0.0 `wire::v1`, big-endian, checked in its source):
//!
//! ```text
//! "PNAU" | major u8 (1) | minor u8 | trailing_len u8 | trailing [u8]
//! proof tag u8 (0 = WormholeMerkle)
//! vaa_len u16 | vaa [u8]
//! num_updates u8, then per update: message_len u16 | message [u8] | proof_len u8 | [u8; 20] * proof_len
//! ```
//!
//! A `PriceFeedMessage` (`messages.rs`) is tag `0` then `feed_id [u8; 32]`, `price i64`, `conf
//! u64`, `exponent i32`, `publish_time i64`, `prev_publish_time i64`, `ema_price i64`, `ema_conf
//! u64`, big-endian (85 bytes). A VAA v1 starts with `version u8 (1)`, `guardian_set_index u32`,
//! `num_signatures u8` and 66 bytes per signature.
//!
//! Parse errors are fixed categories: the bytes come from a price source and are never quoted.

use crate::{KeeperError, PrintMsg};

/// The accumulator update magic.
pub const PNAU_MAGIC: [u8; 4] = *b"PNAU";

/// Proof tag of a Wormhole merkle proof, the only one defined.
const WORMHOLE_MERKLE: u8 = 0;

/// Tag of a `PriceFeedMessage`.
const PRICE_FEED_MESSAGE: u8 = 0;

/// Length of a serialized `PriceFeedMessage`.
pub const PRICE_FEED_MESSAGE_LEN: usize = 85;

/// One message of an accumulator update with its merkle proof, as the receiver's `post_update`
/// takes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MerkleUpdate {
    /// The serialized message (a `PriceFeedMessage` for price feeds).
    pub message: Vec<u8>,
    /// Keccak160 sibling hashes from the leaf to the root.
    pub proof: Vec<[u8; 20]>,
}

/// A parsed accumulator update: one VAA over a merkle root, and the messages it proves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccumulatorUpdate {
    /// The VAA signed by the Wormhole guardians.
    pub vaa: Vec<u8>,
    /// The messages and their proofs.
    pub updates: Vec<MerkleUpdate>,
}

impl AccumulatorUpdate {
    /// Parses Hermes' binary update.
    ///
    /// # Errors
    ///
    /// `KeeperError::Update` for a wrong magic or version, an unknown proof type, a truncated
    /// field or trailing bytes.
    pub fn parse(bytes: &[u8]) -> Result<Self, KeeperError> {
        let mut r = Reader(bytes);
        if r.array::<4>()? != PNAU_MAGIC {
            return Err(KeeperError::Update("not an accumulator update"));
        }
        let (major, _minor) = (r.u8()?, r.u8()?);
        if major != 1 {
            return Err(KeeperError::Update(
                "unsupported accumulator update version",
            ));
        }
        let trailing = usize::from(r.u8()?);
        r.take(trailing)?;
        if r.u8()? != WORMHOLE_MERKLE {
            return Err(KeeperError::Update("unknown accumulator proof type"));
        }
        let vaa_len = usize::from(r.u16()?);
        let vaa = r.take(vaa_len)?.to_vec();
        let count = r.u8()?;
        let mut updates = Vec::with_capacity(usize::from(count));
        for _ in 0..count {
            let message_len = usize::from(r.u16()?);
            let message = r.take(message_len)?.to_vec();
            let proof_len = r.u8()?;
            let mut proof = Vec::with_capacity(usize::from(proof_len));
            for _ in 0..proof_len {
                proof.push(r.array::<20>()?);
            }
            updates.push(MerkleUpdate { message, proof });
        }
        if !r.0.is_empty() {
            return Err(KeeperError::Update(
                "trailing bytes after the accumulator update",
            ));
        }
        Ok(Self { vaa, updates })
    }

    /// The update whose `PriceFeedMessage` is for `feed`, with the message decoded.
    #[must_use]
    pub fn find(&self, feed: &[u8; 32]) -> Option<(&MerkleUpdate, PrintMsg)> {
        self.updates
            .iter()
            .find_map(|u| match price_feed_message(&u.message) {
                Ok((id, print)) if &id == feed => Some((u, print)),
                _ => None,
            })
    }

    /// The guardian set index the VAA is signed under.
    ///
    /// # Errors
    ///
    /// `KeeperError::Update` if the VAA is not version 1 or is truncated.
    pub fn guardian_set_index(&self) -> Result<u32, KeeperError> {
        let mut r = Reader(&self.vaa);
        if r.u8()? != 1 {
            return Err(KeeperError::Update("unsupported VAA version"));
        }
        Ok(u32::from_be_bytes(r.array::<4>()?))
    }
}

/// Decodes a `PriceFeedMessage`: its feed id and the print.
///
/// # Errors
///
/// `KeeperError::Update` if `message` is not an 85-byte `PriceFeedMessage`.
pub fn price_feed_message(message: &[u8]) -> Result<([u8; 32], PrintMsg), KeeperError> {
    if message.len() != PRICE_FEED_MESSAGE_LEN {
        return Err(KeeperError::Update("not a price feed message"));
    }
    let mut r = Reader(message);
    if r.u8()? != PRICE_FEED_MESSAGE {
        return Err(KeeperError::Update("not a price feed message"));
    }
    let feed = r.array::<32>()?;
    let price = i64::from_be_bytes(r.array::<8>()?);
    let conf = u64::from_be_bytes(r.array::<8>()?);
    let expo = i32::from_be_bytes(r.array::<4>()?);
    let publish_time = i64::from_be_bytes(r.array::<8>()?);
    let prev_publish_time = i64::from_be_bytes(r.array::<8>()?);
    Ok((
        feed,
        PrintMsg {
            price,
            conf,
            expo,
            publish_time,
            prev_publish_time,
        },
    ))
}

/// A cursor over a byte slice; every read is bounds checked.
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], KeeperError> {
        let (head, rest) = self
            .0
            .split_at_checked(n)
            .ok_or(KeeperError::Update("truncated accumulator update"))?;
        self.0 = rest;
        Ok(head)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], KeeperError> {
        self.take(N)?
            .try_into()
            .map_err(|_| KeeperError::Update("truncated accumulator update"))
    }

    fn u8(&mut self) -> Result<u8, KeeperError> {
        Ok(u8::from_be_bytes(self.array::<1>()?))
    }

    fn u16(&mut self) -> Result<u16, KeeperError> {
        Ok(u16::from_be_bytes(self.array::<2>()?))
    }
}

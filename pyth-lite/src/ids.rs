//! Program ids, the account discriminator and the feed id newtype.

/// Pyth Solana receiver program on mainnet, `rec5EKMGg6MxZYaMdyBfgwp4d5rB9T1VQH5pJv5LtFJ`:
/// the owner of every `PriceUpdateV2` account, push oracle feeds included (checked on the
/// committed mainnet fixtures, `fixtures/*.json`).
pub const RECEIVER_PROGRAM_ID: [u8; 32] = [
    0x0c, 0xb7, 0xfa, 0xbb, 0x52, 0xf7, 0xa6, 0x48, 0xbb, 0x5b, 0x31, 0x7d, 0x9a, 0x01, 0x8b, 0x90,
    0x57, 0xcb, 0x02, 0x47, 0x74, 0xfa, 0xfe, 0x01, 0xe6, 0xc4, 0xdf, 0x98, 0xcc, 0x38, 0x58, 0x81,
];

/// Pyth push oracle program on mainnet, `pythWSnswVUd12oZpeFP8e9CVaEqJg25g1Vtc2biRsT`. Its
/// price feed accounts are PDAs with seeds `[shard_id (u16 LE), feed_id]`; they are owned by
/// [`RECEIVER_PROGRAM_ID`], not by this program.
pub const PUSH_ORACLE_PROGRAM_ID: [u8; 32] = [
    0x0c, 0x4a, 0xa0, 0x12, 0x8e, 0x95, 0xd3, 0xe1, 0x62, 0x2a, 0xa5, 0x01, 0xc5, 0x85, 0xa9, 0xeb,
    0x07, 0xb3, 0x73, 0x54, 0xc1, 0x08, 0xea, 0x0b, 0x79, 0x1b, 0x45, 0x6d, 0xc7, 0xee, 0xa3, 0x36,
];

/// Anchor account discriminator of `PriceUpdateV2`: `sha256("account:PriceUpdateV2")[..8]`.
pub const PRICE_UPDATE_V2_DISCRIMINATOR: [u8; 8] = [0x22, 0xf1, 0x23, 0x63, 0x9d, 0x7e, 0xf4, 0xcd];

/// Allocated size of a `PriceUpdateV2` account in bytes (`PriceUpdateV2::LEN`, sized for the
/// 2-byte `Partial` variant).
pub const PRICE_UPDATE_V2_LEN: usize = 134;

/// A Pyth price feed id (32 bytes, shown as hex by Hermes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct FeedId([u8; 32]);

impl FeedId {
    /// Wraps the 32 raw bytes of a feed id.
    #[inline]
    #[must_use]
    pub const fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// The raw bytes.
    #[inline]
    #[must_use]
    pub const fn to_bytes(self) -> [u8; 32] {
        self.0
    }
}

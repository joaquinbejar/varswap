//! Client SDK for varswap: a builder for every program instruction, the PDA helpers and the
//! account decoders, on the program's own layouts (`varswap_core::layout`).
//!
//! Instruction data is a one-byte tag followed by little-endian fields, as each handler in
//! `program/src/` documents it, encoded here field by field (not through the program's `Pod`
//! payloads, so the program's LiteSVM tests check the two encodings against each other);
//! account lists follow the handlers' order and writability.

pub mod decode;
pub mod ix;
pub mod pda;

pub use solana_address::Address;
pub use varswap_core as core;

/// SPL Token.
pub const TOKEN_PROGRAM: Address =
    Address::from_str_const("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");

/// The associated token account program.
pub const ATA_PROGRAM: Address =
    Address::from_str_const("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");

/// The system program.
pub const SYSTEM_PROGRAM: Address = Address::from_str_const("11111111111111111111111111111111");

/// The upgradeable BPF loader.
pub const UPGRADEABLE_LOADER: Address =
    Address::from_str_const("BPFLoaderUpgradeab1e11111111111111111111111");

/// The Pyth receiver program (mainnet and devnet).
pub const PYTH_RECEIVER: Address = Address::new_from_array(pyth_lite::RECEIVER_PROGRAM_ID);

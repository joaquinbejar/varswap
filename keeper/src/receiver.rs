//! Instructions that turn a Hermes update into a `Full` `PriceUpdateV2`: Wormhole's encoded VAA
//! (create, write, verify against the guardian set, close) and the Pyth receiver's
//! `post_update` and `reclaim_rent`.
//!
//! Layouts are Anchor's: an 8-byte discriminator `sha256("global:<name>")[..8]`, then the Borsh
//! arguments. Checked against the sources on 2026-10-30:
//!
//! - pyth-solana-receiver-sdk 2.0.0 (`cpi/accounts.rs`, `cpi/mod.rs`, `lib.rs`): `post_update`
//!   is `[133, 95, 207, 175, 11, 79, 118, 44]` with `PostUpdateParams { merkle_price_update:
//!   { message: Vec<u8>, proof: Vec<[u8; 20]> }, treasury_id: u8 }` and accounts payer (s, w),
//!   encoded_vaa, config, treasury (w), price_update_account (s, w), system program,
//!   write_authority (s). `reclaim_rent` takes payer (s, w) and the price update (w).
//! - wormhole-core-bridge-solana 0.0.1-alpha.5 (`processor/parse_and_verify_vaa`):
//!   `init_encoded_vaa` (write_authority (s), encoded_vaa (w), owned by Wormhole and sized
//!   `46 + vaa_len`), `write_encoded_vaa { index: u32, data: Vec<u8> }`,
//!   `verify_encoded_vaa_v1` (write_authority (s), draft_vaa (w), guardian_set at
//!   `["GuardianSet", index_be]`) and `close_encoded_vaa` (write_authority (s, w), encoded_vaa
//!   (w)).
//!
//! On devnet and mainnet the receiver's config (`["config"]`) names Wormhole
//! [`WORMHOLE_PROGRAM`] (read on devnet: fee 0, data source chain 26).

use solana_instruction::{AccountMeta, Instruction};
use varswap_sdk::{Address, PYTH_RECEIVER, SYSTEM_PROGRAM};

use crate::accumulator::MerkleUpdate;

/// The Wormhole program the Pyth receiver verifies VAAs with (its config's `wormhole` field).
pub const WORMHOLE_PROGRAM: Address =
    Address::from_str_const("HDwcJBJXjL9FpJ7UBsYBtaDjsBUhuLCUYoz3zr8SWWaQ");

/// The compute budget program.
pub const COMPUTE_BUDGET_PROGRAM: Address =
    Address::from_str_const("ComputeBudget111111111111111111111111111111");

/// Bytes of an encoded VAA account before the VAA: discriminator 8, header 34 (status 1,
/// write authority 32, version 1), length 4.
pub const ENCODED_VAA_HEADER: usize = 46;

/// Space of a `PriceUpdateV2` account (pyth-solana-receiver-sdk `PriceUpdateV2::LEN`).
pub const PRICE_UPDATE_LEN: usize = 134;

/// The receiver's discriminators and seeds.
mod disc {
    pub const POST_UPDATE: [u8; 8] = [133, 95, 207, 175, 11, 79, 118, 44];
    pub const RECLAIM_RENT: [u8; 8] = [218, 200, 19, 197, 227, 89, 192, 22];
    pub const INIT_ENCODED_VAA: [u8; 8] = [209, 193, 173, 25, 91, 202, 181, 218];
    pub const WRITE_ENCODED_VAA: [u8; 8] = [199, 208, 110, 177, 150, 76, 118, 42];
    pub const VERIFY_ENCODED_VAA_V1: [u8; 8] = [103, 56, 177, 229, 240, 103, 68, 73];
    pub const CLOSE_ENCODED_VAA: [u8; 8] = [48, 221, 174, 198, 231, 7, 152, 38];
}

/// The receiver's config PDA, `["config"]`.
#[must_use]
pub fn receiver_config() -> Address {
    Address::find_program_address(&[b"config"], &PYTH_RECEIVER).0
}

/// The receiver's treasury PDA, `["treasury", [id]]`.
#[must_use]
pub fn treasury(id: u8) -> Address {
    Address::find_program_address(&[b"treasury", &[id]], &PYTH_RECEIVER).0
}

/// Wormhole's guardian set PDA, `["GuardianSet", index_be]`.
#[must_use]
pub fn guardian_set(index: u32) -> Address {
    Address::find_program_address(&[b"GuardianSet", &index.to_be_bytes()], &WORMHOLE_PROGRAM).0
}

/// `SetComputeUnitLimit(units)`.
#[must_use]
pub fn compute_unit_limit(units: u32) -> Instruction {
    let mut data = vec![2];
    data.extend_from_slice(&units.to_le_bytes());
    Instruction {
        program_id: COMPUTE_BUDGET_PROGRAM,
        accounts: vec![],
        data,
    }
}

/// System `CreateAccount` of `space` bytes owned by `owner`, funded with `lamports` by `payer`.
#[must_use]
pub fn create_account(
    payer: &Address,
    account: &Address,
    lamports: u64,
    space: u64,
    owner: &Address,
) -> Instruction {
    let mut data = Vec::with_capacity(52);
    data.extend_from_slice(&0_u32.to_le_bytes());
    data.extend_from_slice(&lamports.to_le_bytes());
    data.extend_from_slice(&space.to_le_bytes());
    data.extend_from_slice(owner.as_ref());
    Instruction {
        program_id: SYSTEM_PROGRAM,
        accounts: vec![
            AccountMeta::new(*payer, true),
            AccountMeta::new(*account, true),
        ],
        data,
    }
}

/// `init_encoded_vaa`: marks a freshly created Wormhole-owned account as a draft VAA written by
/// `authority`.
#[must_use]
pub fn init_encoded_vaa(authority: &Address, encoded_vaa: &Address) -> Instruction {
    Instruction {
        program_id: WORMHOLE_PROGRAM,
        accounts: vec![
            AccountMeta::new_readonly(*authority, true),
            AccountMeta::new(*encoded_vaa, false),
        ],
        data: disc::INIT_ENCODED_VAA.to_vec(),
    }
}

/// `write_encoded_vaa`: writes `chunk` at byte `index` of the draft VAA.
#[must_use]
pub fn write_encoded_vaa(
    authority: &Address,
    encoded_vaa: &Address,
    index: u32,
    chunk: &[u8],
) -> Option<Instruction> {
    let len = u32::try_from(chunk.len()).ok()?;
    let mut data = disc::WRITE_ENCODED_VAA.to_vec();
    data.extend_from_slice(&index.to_le_bytes());
    data.extend_from_slice(&len.to_le_bytes());
    data.extend_from_slice(chunk);
    Some(Instruction {
        program_id: WORMHOLE_PROGRAM,
        accounts: vec![
            AccountMeta::new_readonly(*authority, true),
            AccountMeta::new(*encoded_vaa, false),
        ],
        data,
    })
}

/// `verify_encoded_vaa_v1`: checks the guardian signatures against guardian set
/// `guardian_set_index`.
#[must_use]
pub fn verify_encoded_vaa_v1(
    authority: &Address,
    encoded_vaa: &Address,
    guardian_set_index: u32,
) -> Instruction {
    Instruction {
        program_id: WORMHOLE_PROGRAM,
        accounts: vec![
            AccountMeta::new_readonly(*authority, true),
            AccountMeta::new(*encoded_vaa, false),
            AccountMeta::new_readonly(guardian_set(guardian_set_index), false),
        ],
        data: disc::VERIFY_ENCODED_VAA_V1.to_vec(),
    }
}

/// `close_encoded_vaa`: returns the encoded VAA's rent to `authority`.
#[must_use]
pub fn close_encoded_vaa(authority: &Address, encoded_vaa: &Address) -> Instruction {
    Instruction {
        program_id: WORMHOLE_PROGRAM,
        accounts: vec![
            AccountMeta::new(*authority, true),
            AccountMeta::new(*encoded_vaa, false),
        ],
        data: disc::CLOSE_ENCODED_VAA.to_vec(),
    }
}

/// The receiver's `post_update`: writes `update`'s message as a `Full` `PriceUpdateV2` at
/// `price_update` (a fresh signer), proven against the verified `encoded_vaa`. `payer` pays the
/// rent and is the write authority.
#[must_use]
pub fn post_update(
    payer: &Address,
    encoded_vaa: &Address,
    price_update: &Address,
    update: &MerkleUpdate,
    treasury_id: u8,
) -> Option<Instruction> {
    let message_len = u32::try_from(update.message.len()).ok()?;
    let proof_len = u32::try_from(update.proof.len()).ok()?;
    let mut data = disc::POST_UPDATE.to_vec();
    data.extend_from_slice(&message_len.to_le_bytes());
    data.extend_from_slice(&update.message);
    data.extend_from_slice(&proof_len.to_le_bytes());
    for node in &update.proof {
        data.extend_from_slice(node);
    }
    data.push(treasury_id);
    Some(Instruction {
        program_id: PYTH_RECEIVER,
        accounts: vec![
            AccountMeta::new(*payer, true),
            AccountMeta::new_readonly(*encoded_vaa, false),
            AccountMeta::new_readonly(receiver_config(), false),
            AccountMeta::new(treasury(treasury_id), false),
            AccountMeta::new(*price_update, true),
            AccountMeta::new_readonly(SYSTEM_PROGRAM, false),
            AccountMeta::new_readonly(*payer, true),
        ],
        data,
    })
}

/// The receiver's `reclaim_rent`: closes `price_update` (written by `payer`) to `payer`.
#[must_use]
pub fn reclaim_rent(payer: &Address, price_update: &Address) -> Instruction {
    Instruction {
        program_id: PYTH_RECEIVER,
        accounts: vec![
            AccountMeta::new(*payer, true),
            AccountMeta::new(*price_update, false),
        ],
        data: disc::RECLAIM_RENT.to_vec(),
    }
}

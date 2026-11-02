//! Orphan drill (`docs/runbooks/orphaned-vaas.md`): creates an encoded VAA account written by
//! the payer and leaves it open, as a poster killed after the first transaction of a post
//! would. The keeper's leftover scan must then close it. The bytes written are not a VAA (the
//! drill needs only the account); nothing is verified or posted, so no Hermes request is made.
//!
//! ```sh
//! SOLANA_URL=https://api.devnet.solana.com \
//!   cargo run -p pyth-post --example open_encoded_vaa -- <payer keypair file>
//! ```
//!
//! It takes the payer's `PayerLock`, so it refuses to run while a keeper holds that payer.

use std::process::ExitCode;

use pyth_post::{ix, PayerLock, RpcTransport, Transport};
use solana_keypair::Keypair;
use solana_signer::Signer;

/// Bytes written into the account: not a VAA.
const BODY: [u8; 64] = [0; 64];

fn run() -> Result<String, String> {
    let path = std::env::args()
        .nth(1)
        .ok_or("usage: open_encoded_vaa <payer keypair file>")?;
    let url = std::env::var("SOLANA_URL").map_err(|_| "SOLANA_URL is not set")?;
    let mut file =
        std::fs::File::open(&path).map_err(|_| format!("keypair {path} cannot be opened"))?;
    let payer = solana_keypair::read_keypair(&mut file)
        .map_err(|_| format!("keypair {path} is not a JSON array of 64 bytes"))?;
    let _lock = PayerLock::acquire(&payer.pubkey())
        .map_err(|e| format!("payer lock: {e} (is a keeper running on this payer?)"))?;

    let mut chain = RpcTransport::new(&url);
    let vaa = Keypair::new();
    let space = ix::ENCODED_VAA_HEADER
        .checked_add(BODY.len())
        .ok_or("account size overflows")?;
    let rent = chain.rent_exempt(space).map_err(|e| e.to_string())?;
    let space = u64::try_from(space).map_err(|_| "account size overflows")?;
    let instructions = [
        ix::create_account(
            &payer.pubkey(),
            &vaa.pubkey(),
            rent,
            space,
            &ix::WORMHOLE_PROGRAM,
        ),
        ix::init_encoded_vaa(&payer.pubkey(), &vaa.pubkey()),
        ix::write_encoded_vaa(&payer.pubkey(), &vaa.pubkey(), 0, &BODY)
            .ok_or("chunk too long")?,
    ];
    let signature = chain
        .send(&instructions, &payer, Some(&vaa))
        .map_err(|e| e.to_string())?;
    Ok(format!(
        "encoded VAA {} left open, write authority {}, rent {rent} lamports: {signature}",
        vaa.pubkey(),
        payer.pubkey()
    ))
}

fn main() -> ExitCode {
    match run() {
        Ok(line) => {
            println!("{line}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

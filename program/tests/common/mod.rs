//! LiteSVM harness for the varswap program: environment, instruction builders, synthetic
//! Pyth accounts and error-code checks.

// Test harness: LiteSVM's error type is large by design.
#![allow(dead_code, clippy::result_large_err)]

use std::path::PathBuf;

use litesvm::types::{FailedTransactionMetadata, TransactionMetadata};
use litesvm::LiteSVM;
use solana_account::Account;
use solana_address::Address;
use solana_clock::Clock;
use solana_instruction::{AccountMeta, Instruction};
use solana_keypair::Keypair;
use solana_signer::Signer;
use solana_transaction::Transaction;
use varswap_program::layout::{CreateWindowArgs, SetFeedConfigArgs};

pub const PROGRAM_ID: Address = Address::new_from_array([0x5a; 32]);
pub const LOADER: Address = Address::from_str_const("BPFLoaderUpgradeab1e11111111111111111111111");
pub const SYSTEM: Address = Address::new_from_array([0; 32]);
pub const RECEIVER: Address = Address::new_from_array(pyth_lite::RECEIVER_PROGRAM_ID);
/// Crypto.SOL/USD.
pub const SOL: [u8; 32] = hex32("ef0d8b6fda2ceba41da15d4095d1da392a0d2f8ed0c6c7bc0f4cfac8c280b56d");
/// Metal.XAU/USD.
pub const XAU: [u8; 32] = hex32("765d2ba906dbc32ca17cc11f5310a89e9ee1f6420508c63861f2f8ba4ee34bb2");

pub const H: u32 = 3_600;
/// v1 parameters: 6 h outage guard, 24 h backfill, 10% confidence, 30 days hourly.
pub const TOLERANCE: u32 = 6 * H;
pub const BACKFILL: u32 = 24 * H;
pub const MAX_CONF: u16 = 1_000;
pub const MAX_BUCKETS: u32 = 721;
pub const T0: i64 = 1_800_000_000;

const fn hex32(s: &str) -> [u8; 32] {
    let b = s.as_bytes();
    let mut out = [0u8; 32];
    let mut i = 0;
    while i < 32 {
        out[i] = nib(b[2 * i]) * 16 + nib(b[2 * i + 1]);
        i += 1;
    }
    out
}

const fn nib(c: u8) -> u8 {
    match c {
        b'0'..=b'9' => c - b'0',
        _ => c - b'a' + 10,
    }
}

/// Custom error codes of the program (`program/src/error.rs`), restated so the tests do not
/// reuse the program's own mapping.
pub mod code {
    pub const INVALID_INSTRUCTION: u32 = 100;
    pub const INVALID_INSTRUCTION_DATA: u32 = 101;
    pub const NOT_ENOUGH_ACCOUNTS: u32 = 102;
    pub const MISSING_SIGNATURE: u32 = 103;
    pub const NOT_WRITABLE: u32 = 104;
    pub const INVALID_ACCOUNT_OWNER: u32 = 105;
    pub const INVALID_PDA: u32 = 106;
    pub const INVALID_ACCOUNT_DATA: u32 = 107;
    pub const INVALID_SYSTEM_PROGRAM: u32 = 108;
    pub const NOT_AUTHORITY: u32 = 109;
    pub const ALREADY_INITIALIZED: u32 = 110;
    pub const INVALID_FEED_CONFIG: u32 = 111;
    pub const INTERVAL_BELOW_MINIMUM: u32 = 112;
    pub const WINDOW_NOT_FINAL: u32 = 113;
    pub const WINDOW_STARTED: u32 = 114;
    pub const SWAP_ALREADY_SETTLED: u32 = 115;
    pub const SWAP_NOT_SETTLED: u32 = 116;
    pub const ALREADY_CLAIMED: u32 = 117;
    pub const NOT_CLAIMED: u32 = 118;
    pub const INVALID_MINT: u32 = 119;
    pub const INVALID_TOKEN_PROGRAM: u32 = 120;
    pub const INVALID_TOKEN_ACCOUNT: u32 = 121;
    pub const INVALID_VAULT: u32 = 122;
    pub const WRONG_WINDOW: u32 = 123;
    pub const WRONG_RECEIVER: u32 = 124;
    // 140 + PythError.
    pub const ORACLE_WRONG_OWNER: u32 = 140;
    pub const ORACLE_WRONG_DISCRIMINATOR: u32 = 141;
    pub const ORACLE_TRUNCATED: u32 = 142;
    pub const ORACLE_UNKNOWN_LEVEL: u32 = 143;
    pub const ORACLE_NOT_FULL: u32 = 144;
    pub const ORACLE_WRONG_FEED: u32 = 145;
    pub const ORACLE_NON_POSITIVE: u32 = 146;
    pub const ORACLE_CONF: u32 = 148;
    // 160 + CoreError::code.
    pub const EMPTY_WINDOW: u32 = 160;
    pub const ZERO_INTERVAL: u32 = 161;
    pub const END_OFF_GRID: u32 = 162;
    pub const TOO_MANY_BUCKETS: u32 = 163;
    pub const ZERO_TOLERANCE: u32 = 164;
    pub const ZERO_BACKFILL: u32 = 165;
    pub const INVALID_MAX_CONF: u32 = 166;
    pub const START_NOT_IN_FUTURE: u32 = 167;
    pub const EXPONENT_OUT_OF_RANGE: u32 = 168;
    pub const NO_PREVIOUS_PRINT: u32 = 170;
    pub const NOT_ON_GRID: u32 = 172;
    pub const BUCKET_ALREADY_SAMPLED: u32 = 173;
    pub const BACKFILL_CLOSED: u32 = 174;
    pub const WINDOW_FINAL: u32 = 175;
    pub const FINALIZE_TOO_EARLY: u32 = 176;
    pub const EXPONENT_MISMATCH: u32 = 180;
    pub const ZERO_NOTIONAL: u32 = 181;
    pub const INVALID_STRIKE: u32 = 182;
    pub const INVALID_CAP: u32 = 183;
    pub const NOTIONAL_ABOVE_ERROR_BOUND: u32 = 184;
    pub const COLLATERAL_OVERFLOW: u32 = 185;
    pub const CORE_INVALID_ACCOUNT_DATA: u32 = 177;
}

pub fn program_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target/deploy/varswap_program.so")
}

thread_local! {
    /// The `.so` that [`Env::new`] loads on this thread; the CU bench swaps in the build
    /// without events.
    static SO: std::cell::RefCell<PathBuf> = std::cell::RefCell::new(program_path());
}

/// Makes later [`Env::new`] calls on this thread load `path`.
pub fn use_program(path: PathBuf) {
    SO.with(|s| *s.borrow_mut() = path);
}

pub struct Env {
    pub svm: LiteSVM,
    pub admin: Keypair,
    pub payer: Keypair,
    /// The collateral mint (6 decimals, `admin` as mint authority), set in `Config`.
    pub usdc: Address,
}

pub type Res = Result<TransactionMetadata, FailedTransactionMetadata>;

impl Env {
    /// The program loaded through the upgradeable loader, `admin` as its upgrade authority.
    pub fn new() -> Self {
        let mut svm = LiteSVM::new();
        let path = SO.with(|s| s.borrow().clone());
        let bytes = std::fs::read(&path)
            .unwrap_or_else(|_| panic!("{} missing: run `make program`", path.display()));
        svm.add_program(PROGRAM_ID, &bytes).unwrap();
        let admin = Keypair::new();
        let payer = Keypair::new();
        svm.airdrop(&admin.pubkey(), 10_000_000_000).unwrap();
        svm.airdrop(&payer.pubkey(), 100_000_000_000).unwrap();
        let usdc = litesvm_token::CreateMint::new(&mut svm, &admin)
            .decimals(6)
            .send()
            .unwrap();
        let mut env = Self {
            svm,
            admin,
            payer,
            usdc,
        };
        let authority = env.admin.pubkey();
        env.set_upgrade_authority(Some(authority));
        env.set_time(T0);
        env
    }

    pub fn program_data() -> Address {
        Address::find_program_address(&[PROGRAM_ID.as_ref()], &LOADER).0
    }

    /// Rewrites the `ProgramData` metadata: `Option<Address>` at bytes 12..45.
    pub fn set_upgrade_authority(&mut self, authority: Option<Address>) {
        let pd = Self::program_data();
        let mut acc = self.svm.get_account(&pd).unwrap();
        match authority {
            Some(a) => {
                acc.data[12] = 1;
                acc.data[13..45].copy_from_slice(a.as_ref());
            }
            None => {
                acc.data[12] = 0;
                acc.data[13..45].fill(0);
            }
        }
        self.svm.set_account(pd, acc).unwrap();
    }

    pub fn set_time(&mut self, t: i64) {
        let mut c: Clock = self.svm.get_sysvar();
        c.unix_timestamp = t;
        c.slot += 1;
        self.svm.set_sysvar(&c);
    }

    pub fn send(&mut self, ixs: &[Instruction], signers: &[&Keypair]) -> Res {
        self.svm.expire_blockhash();
        let payer = signers[0].pubkey();
        let tx = Transaction::new_signed_with_payer(
            ixs,
            Some(&payer),
            signers,
            self.svm.latest_blockhash(),
        );
        self.svm.send_transaction(tx)
    }

    pub fn account(&self, a: &Address) -> Account {
        self.svm.get_account(a).unwrap()
    }

    pub fn set(&mut self, a: Address, owner: Address, data: Vec<u8>) {
        let lamports = self.svm.minimum_balance_for_rent_exemption(data.len());
        self.svm
            .set_account(
                a,
                Account {
                    lamports,
                    data,
                    owner,
                    executable: false,
                    rent_epoch: 0,
                },
            )
            .unwrap();
    }

    // ---- flows --------------------------------------------------------------------------

    pub fn init_config(&mut self) -> Res {
        let ix = ix_init_config(&self.admin.pubkey(), &self.payer.pubkey(), &self.usdc);
        let (p, a) = (self.payer.insecure_clone(), self.admin.insecure_clone());
        self.send(&[ix], &[&p, &a])
    }

    pub fn set_feed(&mut self, args: SetFeedConfigArgs) -> Res {
        let ix = ix_set_feed_config(&self.admin.pubkey(), &self.payer.pubkey(), &args);
        let (p, a) = (self.payer.insecure_clone(), self.admin.insecure_clone());
        self.send(&[ix], &[&p, &a])
    }

    pub fn create_window(&mut self, feed: [u8; 32], start: i64, end: i64, interval: u32) -> Res {
        let ix = ix_create_window(&self.payer.pubkey(), feed, start, end, interval);
        let p = self.payer.insecure_clone();
        self.send(&[ix], &[&p])
    }

    /// Posts `update` at a fresh address owned by the receiver and samples it.
    pub fn sample(&mut self, window: &Address, update: &PriceUpdate) -> Res {
        let at = Address::new_unique();
        self.set(at, RECEIVER, update.bytes());
        self.sample_at(window, &at)
    }

    pub fn sample_at(&mut self, window: &Address, update: &Address) -> Res {
        let ix = ix_sample(window, update);
        let p = self.payer.insecure_clone();
        self.send(&[ix], &[&p])
    }

    pub fn finalize(&mut self, window: &Address) -> Res {
        let ix = ix_finalize(window);
        let p = self.payer.insecure_clone();
        self.send(&[ix], &[&p])
    }

    /// Config, the SOL feed with the v1 parameters, and one hourly window of `n` intervals
    /// starting at `T0 + 1 h`.
    pub fn with_window(n: u32) -> (Self, Address) {
        let mut env = Self::new();
        env.init_config().unwrap();
        env.set_feed(feed_args(SOL)).unwrap();
        let start = T0 + i64::from(H);
        let end = start + i64::from(H) * i64::from(n);
        env.create_window(SOL, start, end, H).unwrap();
        (env, window_pda(SOL, start, end, H))
    }
}

pub fn feed_args(feed: [u8; 32]) -> SetFeedConfigArgs {
    SetFeedConfigArgs {
        feed,
        receiver_program: pyth_lite::RECEIVER_PROGRAM_ID,
        min_interval: 60,
        tolerance: TOLERANCE,
        backfill_deadline: BACKFILL,
        max_buckets: MAX_BUCKETS,
        max_conf: MAX_CONF,
        exponent: -8,
        _pad: [0; 1],
    }
}

pub fn config_pda() -> Address {
    Address::find_program_address(&[b"config"], &PROGRAM_ID).0
}

pub fn feed_pda(feed: [u8; 32]) -> Address {
    Address::find_program_address(&[b"feed", &feed], &PROGRAM_ID).0
}

pub fn window_pda(feed: [u8; 32], start: i64, end: i64, interval: u32) -> Address {
    Address::find_program_address(
        &[
            b"window",
            &feed,
            &start.to_le_bytes(),
            &end.to_le_bytes(),
            &interval.to_le_bytes(),
        ],
        &PROGRAM_ID,
    )
    .0
}

pub fn ix_init_config(admin: &Address, payer: &Address, mint: &Address) -> Instruction {
    let mut data = vec![0];
    data.extend_from_slice(mint.as_ref());
    Instruction::new_with_bytes(
        PROGRAM_ID,
        &data,
        vec![
            AccountMeta::new_readonly(*admin, true),
            AccountMeta::new(*payer, true),
            AccountMeta::new(config_pda(), false),
            AccountMeta::new_readonly(Env::program_data(), false),
            AccountMeta::new_readonly(SYSTEM, false),
        ],
    )
}

pub fn ix_set_feed_config(admin: &Address, payer: &Address, a: &SetFeedConfigArgs) -> Instruction {
    let mut data = vec![1];
    data.extend_from_slice(bytemuck::bytes_of(a));
    Instruction::new_with_bytes(
        PROGRAM_ID,
        &data,
        vec![
            AccountMeta::new_readonly(*admin, true),
            AccountMeta::new(*payer, true),
            AccountMeta::new_readonly(config_pda(), false),
            AccountMeta::new(feed_pda(a.feed), false),
            AccountMeta::new_readonly(SYSTEM, false),
        ],
    )
}

pub fn ix_create_window(
    payer: &Address,
    feed: [u8; 32],
    start: i64,
    end: i64,
    interval: u32,
) -> Instruction {
    let mut data = vec![2];
    data.extend_from_slice(bytemuck::bytes_of(&CreateWindowArgs {
        start,
        end,
        interval,
        _pad: [0; 4],
    }));
    Instruction::new_with_bytes(
        PROGRAM_ID,
        &data,
        vec![
            AccountMeta::new(*payer, true),
            AccountMeta::new_readonly(feed_pda(feed), false),
            AccountMeta::new(window_pda(feed, start, end, interval), false),
            AccountMeta::new_readonly(SYSTEM, false),
        ],
    )
}

pub fn ix_sample(window: &Address, update: &Address) -> Instruction {
    Instruction::new_with_bytes(
        PROGRAM_ID,
        &[3],
        vec![
            AccountMeta::new(*window, false),
            AccountMeta::new_readonly(*update, false),
        ],
    )
}

pub fn ix_finalize(window: &Address) -> Instruction {
    Instruction::new_with_bytes(PROGRAM_ID, &[4], vec![AccountMeta::new(*window, false)])
}

/// A synthetic `PriceUpdateV2` account (layout: pyth-solana-receiver-sdk 2.0.0).
#[derive(Debug, Clone, Copy)]
pub struct PriceUpdate {
    pub partial: Option<u8>,
    pub feed: [u8; 32],
    pub price: i64,
    pub conf: u64,
    pub exponent: i32,
    pub publish_time: i64,
    pub prev_publish_time: i64,
}

impl PriceUpdate {
    /// A SOL print of `price` (exponent −8, conf 0.1%) published `publish` with `prev`.
    pub fn sol(price: i64, publish: i64, prev: i64) -> Self {
        Self {
            partial: None,
            feed: SOL,
            price,
            conf: (price / 1_000).max(0) as u64,
            exponent: -8,
            publish_time: publish,
            prev_publish_time: prev,
        }
    }

    pub fn bytes(&self) -> Vec<u8> {
        let mut d = pyth_lite::PRICE_UPDATE_V2_DISCRIMINATOR.to_vec();
        d.extend_from_slice(&[0x11; 32]);
        match self.partial {
            Some(n) => d.extend_from_slice(&[0, n]),
            None => d.push(1),
        }
        d.extend_from_slice(&self.feed);
        d.extend_from_slice(&self.price.to_le_bytes());
        d.extend_from_slice(&self.conf.to_le_bytes());
        d.extend_from_slice(&self.exponent.to_le_bytes());
        d.extend_from_slice(&self.publish_time.to_le_bytes());
        d.extend_from_slice(&self.prev_publish_time.to_le_bytes());
        d.extend_from_slice(&self.price.to_le_bytes());
        d.extend_from_slice(&self.conf.to_le_bytes());
        d.extend_from_slice(&1u64.to_le_bytes());
        d.resize(pyth_lite::PRICE_UPDATE_V2_LEN, 0);
        d
    }
}

/// The custom error code of a failed transaction.
pub fn err_code(r: &Res) -> u32 {
    let e = r.as_ref().expect_err("transaction succeeded");
    let s = format!("{:?}", e.err);
    let tail = s
        .split("Custom(")
        .nth(1)
        .unwrap_or_else(|| panic!("not a custom error: {s}\n{}", e.meta.pretty_logs()));
    tail[..tail.find(')').unwrap()].parse().unwrap()
}

pub fn expect_code(r: &Res, code: u32) {
    assert_eq!(
        err_code(r),
        code,
        "{:?}",
        r.as_ref().err().map(|e| &e.meta.logs)
    );
}

/// Any failure (runtime errors that are not custom codes, such as a missing signature).
pub fn expect_fail(r: &Res) {
    assert!(r.is_err(), "transaction succeeded");
}

// ---- swaps ------------------------------------------------------------------------------

pub const TOKEN: Address = Address::from_str_const("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");
pub const ONE_VAR: u128 = 1 << 64;

pub fn swap_pda(window: &Address, long: &Address, short: &Address, index: u64) -> Address {
    Address::find_program_address(
        &[
            b"swap",
            window.as_ref(),
            long.as_ref(),
            short.as_ref(),
            &index.to_le_bytes(),
        ],
        &PROGRAM_ID,
    )
    .0
}

pub fn vault_pda(swap: &Address) -> Address {
    Address::find_program_address(&[b"vault", swap.as_ref()], &PROGRAM_ID).0
}

impl Env {
    /// An associated token account of `mint` for `owner`, funded with `amount` (the admin is
    /// the mint authority).
    pub fn funded(&mut self, mint: &Address, owner: &Address, amount: u64) -> Address {
        let admin = self.admin.insecure_clone();
        let ata = litesvm_token::CreateAssociatedTokenAccount::new(&mut self.svm, &admin, mint)
            .owner(owner)
            .send()
            .unwrap();
        if amount > 0 {
            litesvm_token::MintTo::new(&mut self.svm, &admin, mint, &ata, amount)
                .send()
                .unwrap();
        }
        ata
    }

    pub fn balance(&self, token_account: &Address) -> u64 {
        let data = self.svm.get_account(token_account).unwrap().data;
        u64::from_le_bytes(data[64..72].try_into().unwrap())
    }

    /// Marks a token account frozen (SPL Token `AccountState::Frozen` at byte 108).
    pub fn freeze(&mut self, token_account: &Address) {
        let mut acc = self.svm.get_account(token_account).unwrap();
        acc.data[108] = 2;
        self.svm.set_account(*token_account, acc).unwrap();
    }

    pub fn new_mint(&mut self) -> Address {
        let admin = self.admin.insecure_clone();
        litesvm_token::CreateMint::new(&mut self.svm, &admin)
            .decimals(6)
            .send()
            .unwrap()
    }
}

/// The accounts and terms of one `open_swap`.
#[derive(Debug, Clone, Copy)]
pub struct Open {
    pub window: Address,
    pub long: Address,
    pub short: Address,
    pub payer: Address,
    pub long_src: Address,
    pub short_src: Address,
    pub mint: Address,
    pub notional: u64,
    pub strike: u128,
    pub cap: u128,
    pub index: u64,
}

impl Open {
    pub fn swap(&self) -> Address {
        swap_pda(&self.window, &self.long, &self.short, self.index)
    }

    pub fn vault(&self) -> Address {
        vault_pda(&self.swap())
    }

    pub fn ix(&self) -> Instruction {
        let mut data = vec![5];
        data.extend_from_slice(&self.notional.to_le_bytes());
        data.extend_from_slice(&self.index.to_le_bytes());
        data.extend_from_slice(&self.strike.to_le_bytes());
        data.extend_from_slice(&self.cap.to_le_bytes());
        Instruction::new_with_bytes(
            PROGRAM_ID,
            &data,
            vec![
                AccountMeta::new_readonly(self.long, true),
                AccountMeta::new_readonly(self.short, true),
                AccountMeta::new(self.payer, true),
                AccountMeta::new_readonly(config_pda(), false),
                AccountMeta::new_readonly(self.window, false),
                AccountMeta::new(self.swap(), false),
                AccountMeta::new(self.vault(), false),
                AccountMeta::new(self.long_src, false),
                AccountMeta::new(self.short_src, false),
                AccountMeta::new_readonly(self.mint, false),
                AccountMeta::new_readonly(TOKEN, false),
                AccountMeta::new_readonly(SYSTEM, false),
            ],
        )
    }
}

pub fn ix_settle(window: &Address, swap: &Address) -> Instruction {
    Instruction::new_with_bytes(
        PROGRAM_ID,
        &[6],
        vec![
            AccountMeta::new_readonly(*window, false),
            AccountMeta::new(*swap, false),
        ],
    )
}

pub fn ix_claim(long: bool, swap: &Address, dest: &Address, mint: &Address) -> Instruction {
    Instruction::new_with_bytes(
        PROGRAM_ID,
        &[if long { 7 } else { 8 }],
        vec![
            AccountMeta::new(*swap, false),
            AccountMeta::new(vault_pda(swap), false),
            AccountMeta::new(*dest, false),
            AccountMeta::new_readonly(*mint, false),
            AccountMeta::new_readonly(TOKEN, false),
        ],
    )
}

pub fn ix_close(swap: &Address, receiver: &Address) -> Instruction {
    Instruction::new_with_bytes(
        PROGRAM_ID,
        &[9],
        vec![
            AccountMeta::new(*swap, false),
            AccountMeta::new(vault_pda(swap), false),
            AccountMeta::new(*receiver, false),
            AccountMeta::new_readonly(TOKEN, false),
        ],
    )
}

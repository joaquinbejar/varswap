# Error codes

Every failure of the varswap program is a `ProgramError::Custom(code)`. Codes are stable
once the program is deployed; they were re-ranged in milestone 4a, before any deployment
(`program/src/error.rs`). `program/tests/config.rs` checks that every code below is listed.

| Code | Name | Source | Meaning |
| ---: | --- | --- | --- |
| 100 | `InvalidInstruction` | `VarswapError` | Empty instruction data or an unassigned tag. |
| 101 | `InvalidInstructionData` | `VarswapError` | The payload has the wrong length or invalid padding. |
| 102 | `NotEnoughAccounts` | `VarswapError` | Fewer accounts than the instruction needs. |
| 103 | `MissingSignature` | `VarswapError` | A required signer did not sign. |
| 104 | `NotWritable` | `VarswapError` | An account that is written is not writable. |
| 105 | `InvalidAccountOwner` | `VarswapError` | An account is owned by the wrong program. |
| 106 | `InvalidPda` | `VarswapError` | An account is not at the expected PDA. |
| 107 | `InvalidAccountData` | `VarswapError` | An account's bytes are not a valid account of the expected kind. |
| 108 | `InvalidSystemProgram` | `VarswapError` | The system program account is not the system program. |
| 109 | `NotAuthority` | `VarswapError` | The signer is not the config admin, or not the program's upgrade authority. |
| 110 | `AlreadyInitialized` | `VarswapError` | The account to create already exists. |
| 111 | `InvalidFeedConfig` | `VarswapError` | `set_feed_config` parameters out of range (`min_interval`, `max_buckets`, receiver). |
| 112 | `IntervalBelowMinimum` | `VarswapError` | `create_window` interval below the feed's `min_interval`. |
| 113 | `WindowNotFinal` | `VarswapError` | `settle_swap` on a window that is neither `Finalized` nor `Degraded`. |
| 114 | `WindowStarted` | `VarswapError` | `open_swap` once the window's sampling started (`now >= start`) or it is final. |
| 115 | `SwapAlreadySettled` | `VarswapError` | `settle_swap` on a swap already settled. |
| 116 | `SwapNotSettled` | `VarswapError` | A claim on a swap not settled yet. |
| 117 | `AlreadyClaimed` | `VarswapError` | That side already claimed. |
| 118 | `NotClaimed` | `VarswapError` | `close_swap` before both sides claimed. |
| 119 | `InvalidMint` | `VarswapError` | The mint is not the configured collateral mint, or not the swap's. |
| 120 | `InvalidTokenProgram` | `VarswapError` | The token program is not SPL Token, or not the swap's. |
| 121 | `InvalidTokenAccount` | `VarswapError` | A token account of the wrong mint or owner. |
| 122 | `InvalidVault` | `VarswapError` | The vault is not the swap's `["vault", swap]` PDA. |
| 123 | `WrongWindow` | `VarswapError` | The window is not the swap's. |
| 124 | `WrongReceiver` | `VarswapError` | The rent receiver is not the payer recorded in the swap. |
| 140 | `WrongOwner` | `PythError` (140 + 0) | The account is not owned by the expected Pyth receiver program. |
| 141 | `WrongDiscriminator` | `PythError` (140 + 1) | The first 8 bytes are not the `PriceUpdateV2` Anchor discriminator. |
| 142 | `Truncated` | `PythError` (140 + 2) | The data ends before the last field of the layout. |
| 143 | `UnknownVerificationLevel` | `PythError` (140 + 3) | The `verification_level` tag is neither `Partial` (0) nor `Full` (1). |
| 144 | `NotFullyVerified` | `PythError` (140 + 4) | The update was posted with `Partial` verification; only `Full` is accepted. |
| 145 | `WrongFeed` | `PythError` (140 + 5) | The update is for another feed. |
| 146 | `NonPositivePrice` | `PythError` (140 + 6) | The price is zero or negative. |
| 147 | `Stale` | `PythError` (140 + 7) | `publish_time` is older than `now − max_age`. |
| 148 | `ConfidenceTooWide` | `PythError` (140 + 8) | `conf / price` exceeds the configured bound. |
| 149 | `ArithmeticOverflow` | `PythError` (140 + 9) | A checked arithmetic step overflowed. |
| 160 | `EmptyWindow` | `CoreError` (160 + 0) | `start >= end`. |
| 161 | `ZeroInterval` | `CoreError` (160 + 1) | `interval == 0`. |
| 162 | `EndOffGrid` | `CoreError` (160 + 2) | `end − start` is not a multiple of `interval`. |
| 163 | `TooManyBuckets` | `CoreError` (160 + 3) | `N + 1` exceeds [`crate::MAX_BUCKETS`] or the feed's `max_buckets`. |
| 164 | `ZeroTolerance` | `CoreError` (160 + 4) | `tolerance == 0`. |
| 165 | `ZeroBackfillDeadline` | `CoreError` (160 + 5) | `backfill_deadline == 0`. |
| 166 | `InvalidMaxConf` | `CoreError` (160 + 6) | `max_conf` is 0 or above 10,000 bps. |
| 167 | `StartNotInFuture` | `CoreError` (160 + 7) | `start` is not after the creation time. |
| 168 | `ExponentOutOfRange` | `CoreError` (160 + 8) | The Pyth exponent does not fit in an `i8`. |
| 169 | `PriceOutOfRange` | `CoreError` (160 + 9) | The price mantissa does not fit in an `i64`. |
| 170 | `NoPreviousPrint` | `CoreError` (160 + 10) | `prev_publish_time == 0`: the first aggregate after an oracle `init_price`, whose interval overlaps earlier prints (4.1). |
| 171 | `WrongFeed` | `CoreError` (160 + 11) | The print is for another feed. |
| 172 | `NotOnGrid` | `CoreError` (160 + 12) | The print is canonical for no bucket of the grid. |
| 173 | `BucketAlreadySampled` | `CoreError` (160 + 13) | A bucket of the print's run is already filled. |
| 174 | `BackfillClosed` | `CoreError` (160 + 14) | `now > end + backfill_deadline`: no more samples. |
| 175 | `WindowFinal` | `CoreError` (160 + 15) | The window is already `Finalized` or `Degraded`. |
| 176 | `FinalizeTooEarly` | `CoreError` (160 + 16) | Some bucket is empty and `now <= end + backfill_deadline`. |
| 177 | `InvalidAccountData` | `CoreError` (160 + 17) | The account bytes are not a valid window (size, tag, version, state, counters). |
| 178 | `Overflow` | `CoreError` (160 + 18) | A checked arithmetic step overflowed. |
| 220 + k | `Kernel(k)` | `CoreError::Kernel` | The return kernel failed; the payload is its own error code. (`k` is the optfix `PricingError` code: 0 overflow, 1 domain, 2 no convergence, 3 arbitrage bound) |
| 180 | `ExponentMismatch` | `CoreError` (160 + 20) | The print's exponent is not the one the window pinned (`docs/PLAN.md` 4.4). |
| 181 | `ZeroNotional` | `CoreError` (160 + 21) | `notional == 0`. |
| 182 | `InvalidStrike` | `CoreError` (160 + 22) | `strike_var == 0`. |
| 183 | `InvalidCap` | `CoreError` (160 + 23) | `cap_var <= strike_var` or `cap_var` above [`crate::MAX_CAP_VAR`]. |
| 184 | `NotionalAboveErrorBound` | `CoreError` (160 + 24) | `notional × ε >= 2^64`: the realized variance error bound could move a payout by an atom or more (`docs/PLAN.md` 4.4). |
| 185 | `CollateralOverflow` | `CoreError` (160 + 25) | The collateral does not fit in a `u64` of atoms. |

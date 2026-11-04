# ADR 0002: per-sample keeper fees

- Status: proposed (2026-11-04). Reviewed by codex, then ruled on by the SOLANA-PROJECTS
  coordinator; no code before that ruling.
- Context documents: [`docs/PLAN-phase2.md`](../PLAN-phase2.md) P2a, [`docs/PLAN.md`](../PLAN.md)
  4.1 (canonical prints), [ADR 0001](0001-degraded-unwind.md),
  [`docs/audit-scope.md`](../audit-scope.md).

## Summary

Recommendation: an optional fee vault, a new program-owned account per (window, funder), paid
per `sample` call that fills a bucket, to the update's `write_authority`. No window changes
layout, so there is no `WINDOW_VERSION` 2, and live windows can be funded as they are.

Cost does not justify this. The series costs 630,000 lamports a day, which an operator can pay
indefinitely. The reason is that today a third party has no way to be paid for keeping a window
alive, and this adds one without touching the settlement path. "No on-chain fee" is the real
alternative, and it is the right ruling if a second keeper is not a goal before the audit.

This ADR departs from the plan's sketch in two places, both from reading the code: the plan
assumed `WINDOW_VERSION` 2, and it assumed a copier could take a keeper's posted update.

## Cost to cover

Measured on devnet from the keeper's payer `5MTV…`: every transaction since the series started
(2026-11-02 17:00Z to 2026-11-04 15:02Z), read with `getTransaction` one request every 3 s, 145
of 145 returned, none failed.

| Item | Transactions | Fee (lamports) | Rent (lamports) |
| --- | ---: | ---: | --- |
| Create the encoded VAA and write its first chunk | 1 | 10,000 | locks 2,367,280 |
| Write the rest and verify (81,876 CU) | 1 | 5,000 | none |
| `post_update`, `sample`, `reclaim_rent`, `close_encoded_vaa` (38,855 to 40,639 CU, 871 bytes) | 1 | 10,000 | returns 2,367,280 |
| **One print** | **3** | **25,000** | **net 0** |
| `finalize` (1,407 CU) | 1 | 5,000 | none |

The sample totals 1,205,000 lamports: 48 prints and one `finalize`. A daily window has 25 prints,
so it costs 25 × 25,000 + 5,000 = **630,000 lamports**, about 0.23 SOL a year for one series. The
keeper also needs 2,367,280 lamports of float while a print is in flight. Priority fees were zero
on devnet; mainnet would add them. Hermes access is a separate cost that no on-chain fee can see.

## The facts that shape the design

- `sample` reads no signer: `accounts` is `[window, price_update, ..]` and the rest are ignored
  (`program/src/processor.rs`, `sample`). The SDK sends exactly two accounts.
- The window's address is `["window", feed, start, end, interval]`, with no fee and no creator in
  the seeds, and `create_window` is permissionless. A window does not store who created it, and
  whoever creates first fixes every field at that address.
- A print is posted and sampled in **one transaction**, and the update account is closed in that
  same transaction (`pyth-post/src/post.rs`, step 3). So the keeper's update is never available
  to a copier: the threat in the plan's sketch does not arise with this keeper.
- The race that does exist: anyone with Hermes access can post the same print under their own
  `write_authority` and land first. That caller did the whole job and paid for it, and canonicity
  means the price is the same either way.
- `post_update` requires `write_authority` as a signer, and an existing update can only be
  rewritten by its own authority (`pyth-solana-receiver-sdk` 2.0.0, `cpi/accounts.rs`). So a
  recipient named by an update has consented, and no caller can name someone else's key.
- `pyth-lite` already decodes `write_authority` (`pyth-lite/src/update.rs`).
- A bucket is filled at most once (`BucketAlreadySampled`, and nothing is written on error), so
  a window takes at most `N + 1` successful `sample` calls: 25 for the series.

## Decision

### Who funds it

**Anyone, into a separate vault.** `fund_fees(window, fee_per_call, amount)` creates or tops up a
program-owned account at `["fees", window, funder]`:

| Field | Size (bytes) | Note |
| --- | ---: | --- |
| `Hdr` (`FEE_VAULT_TAG`, version 1, bump) | 8 | the existing prefix |
| `window` | 32 | checked against the PDA |
| `funder` | 32 | the only key that may reclaim |
| `fee_per_call` | 8 | non zero, fixed at creation |

That is 80 bytes, 1,447,680 lamports of rent, returned on reclaim. The funder signs, and the
window must exist and not be final. The funder is in the seeds so nobody can squat a window's
vault with a useless fee: each funder has their own, and a keeper passes the one it wants.

### What is paid

**Per `sample` call that fills at least one bucket**, not per bucket. A compressed run of `m`
buckets is one print and one posting cost, so per bucket would pay `m` times for the same work
and pay most during an outage. No caller can raise the count: each paying call fills a new
bucket, and delaying a print cannot change which buckets it fills.

To cover the cost a funder would set `fee_per_call` at about 25,200 lamports (25,000 plus 1/25 of
the `finalize`) and fund 25 × that, 630,000 for the series. Those figures are devnet's; the fee is
the funder's choice and the program sets no minimum.

### To whom

**To the update's `write_authority`.** `sample` gains two optional accounts:
`[window, price_update, fee_vault, recipient]`. After `sample_accepted` succeeds:

1. the vault must be owned by the program, have `FEE_VAULT_TAG`, and name this `window`;
2. `recipient` must equal `update.write_authority()`, or the instruction fails;
3. if the vault's balance above its rent floor covers `fee_per_call`, the fee moves from the vault
   to the recipient by direct lamport arithmetic (checked, whole lamports, no division, so no
   rounding); if not, the sample **succeeds and pays nothing**.

`sample` still needs no signer. Liveness never depends on a fee: an empty vault does not fail a
sample, and a two account `sample` works exactly as today.

### Refund

`reclaim_fees`, signed by the funder, once the window is `Finalized` or `Degraded`: it closes the
vault and returns everything to the funder. Windows keep having no close instruction; only the
vault closes. A funder who wants a refund from a window nobody finalized can call the
permissionless `finalize` themselves.

## Abuse and failure

- **A caller who omits the vault.** The bucket is filled and nobody is paid. The caller paid the
  whole cost and the keeper is spared it, so nothing is lost but the keeper's revenue. A keeper
  that loses this race has paid at least 15,000 lamports for its first two transactions plus the
  close; its third fails with `BucketAlreadySampled`, and `post` closes the encoded VAA on that
  path, so the rent comes back.
- **Sampling someone else's update.** Only possible if a poster leaves its update open, which this
  keeper does not. Passing the vault pays the poster, and omitting it pays nobody. A poster who
  separates post and sample accepts that.
- **Self-dealing.** A funder paying their own keeper moves funds between their own keys: no harm.
- **A fee above cost.** Attracts competition, which is the point. It cannot buy a price:
  canonicity fixes the print, and a bucket fills once.
- **Dust fees.** Harmless: the funder's money, and a keeper ignores a vault not worth passing.
- **A recipient below the rent minimum.** Crediting a key that holds less than 890,880 lamports
  in total fails the transaction under the runtime's rent check. Only that caller's transaction
  fails, nothing is written, and anyone can post the print again until the backfill deadline.
  A poster's authority is normally a funded payer.
- **A forged vault.** Rejected by owner, tag and window checks. A window fails the tag check and
  an update fails the owner check. Neither a window nor a vault can be a recipient: a recipient
  must have signed `post_update`, and this program never signs it.
- **Draining.** A vault pays only on a call that filled a bucket, so it pays at most
  `(N + 1) × fee_per_call`, and only the funder can take the rest.

## Compatibility

- Window, swap and feed layouts, `WINDOW_VERSION`, the bucket bitmap, `sum_sq`, `realized_var` and
  settlement do not change. The index reads every window as today.
- Live version 1 windows keep working with two accounts and can also be funded.
- A third account on `sample` is ignored today and would be read as a vault after the upgrade.
  The SDK sends two; a caller with extra accounts on devnet would have to adapt.
- The `sample` event is unchanged, because `cli/src/record.rs` rejects unknown fields; the fee gets
  its own log line, and the record parser must be shown to ignore it.
- Transaction size: the posting transaction is 871 bytes of the 1,232 limit, and the vault adds
  about 34 bytes (the recipient is already the payer).
- `sample`'s cost grows by the checks and the transfer. This is unmeasured until P2b, and the
  posting transaction's compute limit is rechecked then.

## Alternatives rejected

| Alternative | Why not |
| --- | --- |
| **No on-chain fee** | Simplest, and costs 630,000 lamports a day. Rejected only because a third party can never be paid without trusting the operator. It stays the right ruling if that is not a goal. |
| Pool inside the window (`WINDOW_VERSION` 2) | The fee is not in the seeds, so the first creator of an address fixes it and anyone can squat a canonical window at fee zero. Putting it in the seeds splits one spec into many windows and breaks the series' derivation. It needs a new loader branch in audited core code, a stored creator, and cannot fund live windows. |
| Swap openers pay at `open_swap` | The series has no swaps, so the index would stay unfunded, and it adds a path to the most sensitive instruction. |
| Pay per bucket | Pays `m` times for one post, and pays most during an outage. |
| Pay the transaction's signer | `sample` would need a signer, and whoever copies an open update takes the fee. |
| Fee decaying with lateness | A late canonical print still rescues the window, and the print is the same. No reason to pay less for it. |

## Consequences

- Three program changes: `fund_fees`, `reclaim_fees`, and the optional branch in `sample`. The
  audit scope, pinned at `6731d34`, reopens for them, and the build hash changes. The line count
  is unknown until written; `docs/audit-scope.md` gets the recount.
- P2b changes accordingly: no `WINDOW_VERSION` 2; the drill funds a live window's vault and shows
  fees paid equal to fills, a caller naming the wrong recipient rejected, a sample with an empty
  vault succeeding, and one reclaim. The "copier gets no fee" test becomes the wrong recipient
  test, because a copier cannot reach an update this keeper closes in the same transaction.
- `keeper` gains an optional vault argument, and `varswap-watch` an alert for a vault that cannot
  cover the remaining prints. The roll job funds each new window only if the ruling asks for it.
- **What devnet cannot show.** SOL is free there, so the drill proves the mechanics and not that
  a fee attracts a second keeper. Nothing here is evidence of demand.

## What would reopen this

- A second keeper operator appears, or nobody does by the mainnet readiness review. In the second
  case the instructions stay dormant or are removed before the audit is pinned.
- Measured `sample` cost pushes the posting transaction near its compute limit.
- Mainnet priority fees make the fee model per call inadequate.

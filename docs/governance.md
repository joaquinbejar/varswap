# Governance

Who holds which key, on devnet today and in the mainnet plan (`docs/PLAN-improvements.md`, I2).
Nothing here is deployed on mainnet, and no mainnet action is taken by this document.

## Roles

| Role | What it can do | What it cannot do |
| --- | --- | --- |
| Upgrade authority | Replace the program's code (`solana program deploy` / `upgrade`), extend it, move the authority, or make the program immutable (`--final`) | Nothing in the program itself: no instruction reads it after `init_config` |
| Config admin | `set_feed_config`: vet a feed and set its risk parameters (tolerance, backfill deadline, `max_buckets`, `max_conf`, exponent). Windows copy the parameters at creation, so a change never touches existing windows | Move collateral, settle swaps, change a window, rotate itself (there is no admin rotation instruction) |
| Keeper payers | Pay fees for posting Pyth updates and sampling; one poster instance per payer (`pyth-post` contract) | Anything privileged: `sample`, `finalize`, `settle_swap` and the claims are permissionless |
| Hermes API key | Fetch historical Pyth updates (rate limited, shared with metalbook) | Anything on chain |

The config admin is fixed at `init_config`, which only the program's upgrade authority can sign
(`loader::check_upgrade_authority`): the first caller after a deployment cannot take the
program over. There is no instruction to rotate it.

## Devnet today

| Role | Holder | Since |
| --- | --- | --- |
| Upgrade authority | `Ai4e2jSFMV5vqLDGcyxQLf6GRbjeE5m8BiEMuqWiR9hX` (`~/.config/solana/varswap-devnet-upgrade.json` on the Studio, 0600, backed up; recorded by the coordinator) | 2026-11-02, transfer `4BjAJX9h6va6EKSnHo7gZRsgK9upK37GRasBg1N4spcV62V7kAcjcVfVqfLF5osu3bkitxu73AjUomXHw3wxgrL8` |
| Config admin | `77ieYLKspRnGzAt1AK6ZVsttnTY6T7EY2k7CpTZeCPPV` (the deployer, which signed `init_config` as upgrade authority) | 2026-10-30 |
| Keeper payer | `5MTVStedYVor97UjAYU4shkp6RaTiKXEFupfHJN79D1` (`~/.config/solana/varswap-devnet-keeper.json`) | 2026-10-31 |
| Hermes key | `VARSWAP_HERMES_API_KEY` in the environment only, shared with metalbook: varswap 3 requests per minute across all its processes, metalbook 7 | 2026-11-02 allocation |

No keypair is in the repository.

## Mainnet plan

1. A Squads v4 multisig is created before deployment. **Threshold and members are open: the
   user decides them before mainnet.**
2. The program is deployed with the multisig vault as upgrade authority (or the deployer
   transfers it to the vault before anything else, and the transfer is confirmed before step 3).
3. The multisig signs `init_config`, so the config admin is the multisig vault from the start.
   No admin rotation instruction is needed, and none is added.
4. Every upgrade is proposed from a verifiable build ([`docs/verifiable.md`](verifiable.md)):
   the signers rebuild it and check the build hash before approving, and after execution the
   deployed hash (`solana-verify get-program-hash`) is recorded with the commit and signature.
5. Feed vetting (`set_feed_config`) is a multisig proposal too, with the parameters stated in
   the proposal.
6. Freezing the program (`--final`, no further upgrades) is an explicit decision after the
   audit, not a default.
7. Keeper payers are hot keys with small balances, one per poster instance, topped up by
   runbook; a compromised payer can only waste its own fees.
8. Mainnet needs its own Hermes key (a mainnet readiness item); the allocation above is for
   the shared key used on devnet.

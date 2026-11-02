# Suspected key compromise

Act first on what can move value, then rotate. Record every step with times and signatures.

## Keeper payer

Impact: the payer holds only fees; it has no privilege. An attacker can spend its SOL or post
with it (which only helps liveness), or race the keeper's posts.

1. Stop the keeper ([keeper.md](keeper.md)).
2. Move the remaining SOL to a safe key: `solana transfer <safe> ALL --from <payer keypair>`.
3. Generate a new payer, fund it ([payer-topup.md](payer-topup.md)), reinstall the agent with
   `VARSWAP_KEYPAIR` pointing to it. Open encoded VAAs of the old payer can be closed only by
   it: close them first if the key is still usable, otherwise their rent is lost.

## Upgrade authority

Impact: the attacker can replace the program, so every vault is at risk.

1. If the key is still in our hands, immediately move the authority to a fresh key (or, on
   mainnet, to the multisig) with `solana program set-upgrade-authority`, or freeze the
   program with `--final` if that is the decision; confirm the transaction is finalized.
2. Check `solana program show` and `solana-verify get-program-hash`: if the deployed hash is
   not the recorded one, the program was replaced; warn users not to open swaps and
   coordinate with the coordinator and the user.
3. On mainnet the authority is a multisig: one member key compromised means removing that
   member through a proposal before the threshold can be met.

## Config admin

Impact: the admin can vet feeds and change feed parameters for future windows only; existing
windows keep their copies, and no instruction lets the admin move funds. There is no rotation
instruction: stop creating windows on feeds set after the suspected time, and on mainnet the
admin is the multisig vault (`docs/governance.md`).

## Hermes key

Impact: rate limit exhaustion for varswap and metalbook; no on chain effect. Rotate it
([hermes.md](hermes.md)) and tell the coordinator and metalbook.

This runbook is exercised as a dry run only (no key is actually rotated): see
[drills.md](drills.md).

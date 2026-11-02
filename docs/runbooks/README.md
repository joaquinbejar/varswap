# Runbooks

One file per incident: how it is detected, the commands, and how to verify the fix
(`docs/PLAN-improvements.md`, I4). Devnet values; mainnet differs only in addresses and keys
(`docs/governance.md`). Stop processes only by exact PID, never by name: other repositories'
keepers run on the same machine.

| Runbook | When |
| --- | --- |
| [keeper.md](keeper.md) | install, restart, stop or check a keeper |
| [payer-topup.md](payer-topup.md) | a keeper payer is low on SOL |
| [hermes.md](hermes.md) | Hermes answers 429, or the API key must be rotated |
| [orphaned-vaas.md](orphaned-vaas.md) | an encoded VAA stays open after a crash |
| [backfill.md](backfill.md) | a window has missing buckets before its backfill deadline |
| [degraded-window.md](degraded-window.md) | a window degraded: settle and unwind its swaps |
| [program-upgrade.md](program-upgrade.md) | deploying a new program build |
| [key-compromise.md](key-compromise.md) | a payer, the upgrade authority, the admin or the Hermes key may be compromised |

Drills run on devnet, with their times: [drills.md](drills.md).

Common environment:

```sh
export SOLANA_URL=https://api.devnet.solana.com
export VARSWAP_PROGRAM_ID=BWqsaXvASrbErApP1ovz7vSvBLYqXGaMaZANEYf1xrB9
export VARSWAP_KEYPAIR=~/.config/solana/varswap-devnet-keeper.json   # the keeper payer
# VARSWAP_HERMES_API_KEY comes from ~/.zshrc: never on a command line, in a file or a log
cargo build --release -p varswap-keeper -p varswap-cli
```

# pyth-lite

`no_std` reader for Pyth `PriceUpdateV2` accounts, written for Pinocchio programs. It has no
Anchor dependency, no allocator, no `unsafe` code and no Solana dependency: the caller passes
the account's owner and data as bytes.

- `PriceUpdate::load` checks the owner (the Pyth receiver) and the Anchor discriminator, then
  decodes the Borsh layout by offset. `Partial` and `Full` verification levels have different
  lengths, so every offset is computed.
- `PriceUpdate::verify` runs the price checks and returns an `OraclePrice`: `Full`
  verification, feed id, positive price, staleness and confidence.

The layout follows pyth-solana-receiver-sdk 2.0.0. It is cross-checked against Anchor's own
deserializer on real accounts dumped from mainnet.

```toml
[dependencies]
pyth-lite = "0.1"
```

Part of [varswap](https://github.com/joaquinbejar/varswap), MIT licensed.

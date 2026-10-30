# varswap-sdk

Client SDK for the [varswap](https://github.com/joaquinbejar/varswap) program, built on the
program's own layouts (`varswap_core::layout`):

- `ix`: a builder for every instruction, each matching the program's test harness encoding
  byte for byte;
- `pda`: every PDA (config, feed config, window, swap, vault, ATA, program data);
- `decode`: account decoders with the program's validation (owner, tag, version, layout).

It builds instructions with `solana-address` 2.x and `solana-instruction` 3.x, the generation
LiteSVM 0.18 uses.

```toml
[dependencies]
varswap-sdk = "0.1"
```

The program is on devnet only. Its addresses are in
[docs/devnet.md](https://github.com/joaquinbejar/varswap/blob/main/docs/devnet.md). MIT
licensed.

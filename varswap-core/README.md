# varswap-core

The realized variance accumulator over canonical Pyth prints, and the variance swap terms
settled against it. It is `no_std`, with no allocator, no floating point and no `unsafe`, and
builds for SBF.

- `WindowSpec` sets a sampling grid `b_k = start + k × interval`. Bucket `k` takes the unique
  print with `prev_publish_time < b_k <= publish_time <= b_k + tolerance`.
- `accept_print` is the acceptance rule that the varswap program and its keeper share: feed,
  positive price, confidence bound, pinned exponent, canonical run.
- `Window` is a view over the account bytes. It accumulates squared log returns through
  `LnKernel` (optfix `ln`, Q64.64) with a proven error bound `ε`, and settles on
  `V_cons = max(0, V − ε)`.
- `SwapTerms` covers capped, fully collateralized variance swaps.
- `layout` holds every account and payload layout of the varswap program.

The `insecure-test-kernel` feature swaps in an exact integer stand-in for tests. A program
built with it must never be deployed.

```toml
[dependencies]
varswap-core = "0.1"
```

The design and the numbers behind each decision are in
[docs/PLAN.md](https://github.com/joaquinbejar/varswap/blob/main/docs/PLAN.md). MIT licensed.

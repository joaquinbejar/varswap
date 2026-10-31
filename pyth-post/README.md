# pyth-post

Posts Pyth prices on Solana from Hermes updates, off chain and without Anchor:

- `AccumulatorUpdate::parse` reads Hermes' binary update (the Pythnet accumulator update,
  `PNAU`): the VAA and every price message with its merkle proof. `find` picks a feed's message.
- `ix` builds the Wormhole encoded VAA instructions (create, init, write, `verify_encoded_vaa_v1`,
  close) and the Pyth receiver's `post_update` and `reclaim_rent`.
- `post` runs a fixed plan of three transactions over any `Transport`. The first creates the
  encoded VAA and writes it, the second verifies it against the guardian set, and the third posts
  a `Full` `PriceUpdateV2`, runs your instructions on it, and closes both accounts. A post costs
  fees only.
- `RpcTransport` is a `Transport` over `solana-rpc-client`. Its URL is never shown in `Debug` or
  in errors.

```rust,ignore
use pyth_post::{post, AccumulatorUpdate, PostError, RpcTransport};

let mut cluster = RpcTransport::new("https://api.devnet.solana.com");
let update = AccumulatorUpdate::parse(&hermes_binary)?;
let (message, price) = update.find(&feed_id).ok_or(PostError::Update("no such feed"))?;
let signatures = post(&mut cluster, &payer, &update, message, 0, |price_update| {
    vec![my_program_instruction_reading(price_update)]
})?;
```

The instruction layouts follow the pyth-solana-receiver-sdk 2.0.0 and
wormhole-core-bridge-solana 0.0.1-alpha.5 sources. They are tested in LiteSVM against the
receiver and Wormhole programs dumped from devnet, with real Hermes updates. In those tests the
VAA verifies, the update is posted as `Full`, the message comes back intact, and a tampered VAA
or message is rejected.

**Contract:** a payer belongs to one poster instance, which runs its posts and scans one at
a time. Closing a leftover encoded VAA cannot be atomic against another poster's verify, so
there is no guarantee between posters sharing a payer: other processes, threads, or other
instances in the same process. `PayerLock`, a file lock per payer in
`~/.local/state/pyth-post` (`PYTH_POST_LOCK_DIR`), keeps a second process out. Serializing
threads and instances inside one process is the caller's job. `LeftoverScanner`'s 300-block
guard is defense in depth only.

Part of [varswap](https://github.com/joaquinbejar/varswap), MIT licensed.

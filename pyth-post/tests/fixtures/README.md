# pyth-post test fixtures

Dumped from Solana devnet at slot 516772578 (2026-10-30). The tests load them into LiteSVM, so
the posting path runs against the deployed programs, offline. They are excluded from the
published package (`exclude = ["tests/"]`).

| File | What | sha256 |
| --- | --- | --- |
| `pyth_receiver.so` | Pyth receiver `rec5EKMGg6MxZYaMdyBfgwp4d5rB9T1VQH5pJv5LtFJ`, last deployed in slot 491006444 | `543b7a43b58607ba368a5ce76bff9f51408470f1a1e2fb174393dbdf1f5e1a5a` |
| `wormhole.so` | Wormhole `HDwcJBJXjL9FpJ7UBsYBtaDjsBUhuLCUYoz3zr8SWWaQ` (the receiver config's `wormhole`), last deployed in slot 487852630 | `51c30071d5e70492c2c48be869ac29b9f036b5fe2c3552743e01faf4b3160ca2` |
| `receiver_config.bin` | Receiver config `DaWUKXCyXsnzcvLUyeJRWou8KTn7XtadgTsdhJ6RHS7b`: Wormhole as above, data source chain 26, fee 0, 3 minimum signatures | `f8aca67e2ab8d31e9be38bb0434d6e59e06871c80b744e9e0668a16f254c83a4` |
| `guardian_set_1.bin` | Wormhole guardian set 1 `8d9szTd157GKCLcxBqiLUgB7mek3v65rbsy2ErRyjwQ5`: 5 keys, no expiry | `894e8b9fa421a7aa1f07efb8f18781215f3eeb6cd9e2fd6a64d65e59312bf999` |
| `hermes/sol_usd_2026_10_29/{t}.json` | Hermes `GET /v2/updates/price/{t}?ids[]=<SOL/USD>&encoding=base64&parsed=true` for the 25 hourly boundaries `t` of 2026-10-29 UTC, recorded 2026-10-30. Each answer is that boundary's canonical print (`prev_publish_time < t = publish_time`), signed under guardian set 1 with 3 signatures | |

Treasury 0 (`8hQfT7SVhkCrzUSgBq6u2wYEt1sH3xmofZ5ss3YaydZW`) is a system account with
2.002014461 SOL on devnet. The tests recreate it rather than store it.

The program dumps are cut to their ELF length (the end of the last section or segment, which
is followed only by zero padding in `ProgramData`):

```sh
solana program dump rec5EKMGg6MxZYaMdyBfgwp4d5rB9T1VQH5pJv5LtFJ receiver.full.so --url devnet
solana program dump HDwcJBJXjL9FpJ7UBsYBtaDjsBUhuLCUYoz3zr8SWWaQ wormhole.full.so --url devnet
solana account DaWUKXCyXsnzcvLUyeJRWou8KTn7XtadgTsdhJ6RHS7b --url devnet --output-file receiver_config.bin
solana account 8d9szTd157GKCLcxBqiLUgB7mek3v65rbsy2ErRyjwQ5 --url devnet --output-file guardian_set_1.bin
```

The Hermes responses were fetched with the API key in the `Authorization: Bearer` header. The
key is not in any file.

## `metalbook/`

These are copied from metalbook #25 (`metalbook-tests/tests/fixtures/pyth/`, merged as `56a5cb8`)
together with the two receiver tests ported in `tests/metalbook_xau.rs`:

- `hermes_xau_update.json`: an XAU/USD Hermes update recorded by metalbook on 2026-10-30,
  with its parsed fields.
- `receiver_config.json` and `guardian_set_1.json`: metalbook's records of the same devnet
  accounts. The test checks that each `data_base64` equals `receiver_config.bin` and
  `guardian_set_1.bin` above byte for byte.

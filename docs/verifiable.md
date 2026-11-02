# Verifiable build

The program is built for deployment with [`solana-verify`](https://github.com/Ellipsis-Labs/solana-verifiable-build)
in a pinned Docker image, so anyone can rebuild the same bytes from this repository and compare
them with what a cluster runs. Two hashes are kept apart (`docs/PLAN-improvements.md`, I2):

- the **build hash** below: what the current source builds to. CI's `verifiable` job rebuilds
  it on every pull request and fails unless it equals this table, so a change to the program
  (or to its dependencies in `Cargo.lock`) must record its new build hash in the same PR. It
  says what `main` would deploy, not what is deployed;
- the **deployed hash**: what a cluster runs, recorded per cluster with the commit it was built
  from and the upgrade signature (devnet: [`docs/devnet.md`](devnet.md#program-build-and-authority)).
  It changes only with an upgrade and may lag `main`.

| What | Value |
| --- | --- |
| Build hash | `b9336e631b92f0213652a67440376fb9b275f6a5101c2c6ef809d555273386bb` |
| Image | `solanafoundation/solana-verifiable-build:4.3.0@sha256:12fd4c0a0790f0fc41ef74b0cdb6bccc167ba6137eb1adb749bac649481c86bd` (linux/amd64) |
| Toolchain in the image | `cargo-build-sbf 4.3.0`, platform-tools v1.57, Rust 1.95.0 |
| Command | `solana-verify build -b <image> --library-name varswap_program --arch v3` (`make verifiable`), solana-verify 0.5.2 |
| Output | `target/deploy/varswap_program.so`, 92,376 bytes, sha256 `c057d1cf525b747aca453bab6bfd7e9fba94c40dd3ec21c496138c66a021b0dc` |

The build hash is `solana-verify get-executable-hash`: the sha256 of the program bytes with
trailing zero bytes removed, the same hash `solana-verify get-program-hash` reads from a
cluster (where the program data account is padded with zeros).

## Same compiler as CI and the benchmarks

The image's `cargo-build-sbf` is 4.3.0, CI's is 4.5.0, but both drive platform-tools v1.57
with Rust 1.95.0, the SBF compiler `docs/benchmarks.md` records. The two `.so` files are not
byte identical (92,376 bytes for the verifiable build, 90,824 for `make program`; the image
builds under `/build`, among other differences), and no claim is made that their code is the
same. What was checked is behaviour: with the verifiable `.so` in `target/deploy`,
`program/tests/cu.rs` measures every row of `docs/benchmarks.md` with the same CU, and all 81
program tests pass, the 24 print replay included (2026-11-02, at the commit that added this
file). The deployed program is the verifiable `.so` byte for byte (`solana program dump`, cut
to 92,376 bytes; the rest of the program data is zeros).

## Reproduce

```bash
cargo install --locked solana-verify --version 0.5.2
make verifiable-check            # rebuilds in Docker and compares with the table above
solana-verify get-program-hash -u devnet BWqsaXvASrbErApP1ovz7vSvBLYqXGaMaZANEYf1xrB9
```

`make verifiable` overwrites `target/deploy/varswap_program.so`; run `make program` before the
tests to measure the CI build again. On an Apple silicon machine Docker runs the amd64 image
under emulation; the hash is the same.

## Changing the program

A PR that changes the program, `varswap-core`, `pyth-lite` or `Cargo.lock` in a way that
reaches the program runs `make verifiable` and records the new build hash here. Deploying it
is a separate step: an upgrade from the `make verifiable` output, signed by the upgrade
authority ([`docs/governance.md`](governance.md)), then the deployed hash in the cluster's doc.

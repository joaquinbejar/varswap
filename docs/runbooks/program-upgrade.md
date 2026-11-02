# Program upgrade

Only from a verifiable build ([`docs/verifiable.md`](../verifiable.md)), signed by the upgrade
authority ([`docs/governance.md`](../governance.md)); on devnet that is
`~/.config/solana/varswap-devnet-upgrade.json` (`Ai4e2jSF…`).

1. The change is merged with CI green, including `verifiable`, and `docs/verifiable.md` records
   the build hash of the new source.
2. Build and check:
   ```sh
   make verifiable-check        # writes target/deploy/varswap_program.so, checks the hash
   ```
3. Deploy (the CLI extends the program data if the new build is larger):
   ```sh
   solana program deploy target/deploy/varswap_program.so \
     --program-id BWqsaXvASrbErApP1ovz7vSvBLYqXGaMaZANEYf1xrB9 \
     --upgrade-authority ~/.config/solana/varswap-devnet-upgrade.json -u devnet
   ```
4. Verify:
   ```sh
   solana-verify get-program-hash -u devnet BWqsaXvASrbErApP1ovz7vSvBLYqXGaMaZANEYf1xrB9
   solana program show BWqsaXvASrbErApP1ovz7vSvBLYqXGaMaZANEYf1xrB9 -u devnet
   ```
   The hash equals the build hash; the authority is unchanged. Run a keeper on a short window
   and see it finalize.
5. Record the deployed hash, commit, signature and slot in `docs/devnet.md`.

A running keeper keeps working across an upgrade that leaves the instruction and account
layouts unchanged (the 2026-11-02 upgrade, `docs/devnet.md`). After one that changes them,
rebuild the keeper and reinstall its agent ([keeper.md](keeper.md)).

Mainnet: the same steps as a Squads proposal, the signers rebuilding and checking the hash.

# soroban

The mandate contract for Stellar, written in Rust with the Soroban SDK.

What belongs here: the contract source, its unit, property and fuzz tests, and the test snapshots Soroban generates (kept in version control as regression fixtures). Build output in `target/` is not committed.

It implements the rules in `spec/` and must pass every shared vector.

`contracts/factory` deploys every mandate. It is built with the mandate's code hash, the supported assets and the fee address, and none of them can ever change: there is no admin, setter or upgrade. A mandate cannot prove who deployed it, so off-chain code trusts one only when its address recomputes from a trusted factory, the principal and the salt (the contract ID preimage of the factory's address with the salt `SHA-256(principal as an XDR ScVal address || salt)`), or when that factory's `mandate_created` event names it.

`contracts/wallet-factory` deploys principals' passkey wallets. It is built with one smart account code hash, the OpenZeppelin smart account rebuilt from the tag pinned in `scripts/build-smart-account.sh`, and a deny list of operator addresses, and none of them can ever change: there is no admin, setter, upgrade or fee. A wallet's address is the contract ID preimage of the factory's address with the salt `SHA-256(signers_xdr || policies_xdr || salt)`, with the signers sorted, so it commits to the wallet's starting signers and policies and anyone may pay for the deploy. A wallet starts with the account's single `Default` rule, and the factory refuses itself and every denied address as a signer, a signer's verifier, a signer's raw ed25519 key or a policy. Its tests load the pinned account, verifier and policy wasm from `contracts/wallet-factory/fixtures/`, which `scripts/check-wallet-hash.sh` checks against the pins.

Libraries the contracts share live in `crates/`. `crates/mandate-errors` holds `MandateError`, the one error enum every contract returns, with a variant per entry in `spec/errors.json`. A contract depends on it with `mandate-errors = { workspace = true }` and returns `Result<_, MandateError>`; `scripts/check-error-codes.py` keeps the enum and the table in step. These crates are workspace members but not default members: they build no wasm of their own and are compiled into each contract that uses them.

`batch-limits` holds the production contracts to the network limits: the worst batches the mandate accepts, run as compiled wasm through the factory. It is a test-only crate, and CI runs it in its own job. Build the contracts first with `scripts/soroban-build.sh`, then run `cargo test -p batch-limits -- --nocapture`. Re-run it on any protocol upgrade, and re-read the network settings for testnet and mainnet before the spec is frozen.

Time-boxed probes live in `spikes/`. They are workspace members, so `--workspace` lints and tests them with everything else, but not default members, so a plain build only produces the production contracts. They are never deployed. Each one records its findings in the repository's top-level `spikes/` folder.

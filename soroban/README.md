# soroban

The mandate contract for Stellar, written in Rust with the Soroban SDK.

What belongs here: the contract source, its unit, property and fuzz tests, and the test snapshots Soroban generates (kept in version control as regression fixtures). Build output in `target/` is not committed.

It implements the rules in `spec/` and must pass every shared vector.

Libraries the contracts share live in `crates/`. `crates/mandate-errors` holds `MandateError`, the one error enum every contract returns, with a variant per entry in `spec/errors.json`. A contract depends on it with `mandate-errors = { workspace = true }` and returns `Result<_, MandateError>`; `scripts/check-error-codes.py` keeps the enum and the table in step. These crates are workspace members but not default members: they build no wasm of their own and are compiled into each contract that uses them.

Time-boxed probes live in `spikes/`. They are workspace members, so `--workspace` lints and tests them with everything else, but not default members, so a plain build only produces the production contracts. They are never deployed. Each one records its findings in the repository's top-level `spikes/` folder.

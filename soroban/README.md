# soroban

The mandate contract for Stellar, written in Rust with the Soroban SDK.

What belongs here: the contract source, its unit, property and fuzz tests, and the test snapshots Soroban generates (kept in version control as regression fixtures). Build output in `target/` is not committed.

It implements the rules in `spec/` and must pass every shared vector.

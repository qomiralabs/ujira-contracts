# celo

The mandate contract for Celo, written in Solidity and built with Foundry.

What belongs here: the contract source, its unit, fuzz and invariant tests, and deployment scripts. Vendored dependencies live in `celo/lib/`; build output (`celo/out/`, `celo/cache/`) is not committed.

It implements the rules in `spec/` and must pass every shared vector.

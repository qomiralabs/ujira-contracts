# tests

Behavior tests that run the same vectors from `spec/` against both contracts.

A rule that passes on one chain and fails on the other is a failing build. Tests that only concern one contract belong next to that contract, in `soroban/` or `celo/`.

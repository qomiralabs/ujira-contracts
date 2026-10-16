# ujira-contracts

UJIRA mandate contracts for Stellar (Soroban) and Celo (Solidity), built from one shared spec.

## Repository layout

| Folder | What it holds |
| --- | --- |
| `spec/` | The mandate spec, independent of chain and language, and the shared JSON test vectors. |
| `soroban/` | The Stellar mandate contract (Rust, Soroban SDK). |
| `celo/` | The Celo mandate contract (Solidity, Foundry). |
| `tests/` | Shared behavior tests that drive both contracts from the same vectors. |
| `exploits/` | Adversarial exploit tests against both contracts. |
| `audits/` | Internal and external audit reports. |
| `scripts/` | Repository checks, such as the license header check. |

## License headers

Every Rust and Solidity source file starts with these two lines:

```text
// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs
```

The SPDX line comes first, as the Solidity documentation recommends. Use the year the file was created. To check the tree:

```sh
scripts/check-headers.sh
```

It names every tracked `.rs` or `.sol` file that is missing the header. Vendored code in `celo/lib/` and build output are skipped. `scripts/check-headers.test.sh` tests the check itself.

## Local setup

## License

Apache 2.0. See [LICENSE](LICENSE).

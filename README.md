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

### Celo (Foundry)

Install Foundry 1.5.1 with `foundryup`, then fetch the pinned `forge-std` submodule:

```sh
curl -L https://foundry.paradigm.xyz | bash
foundryup --install v1.5.1
git submodule update --init --recursive
```

Check that `forge --version` prints `1.5.1`. A newer local Foundry can format, lint or compile differently from the pinned one, so switch back with `foundryup --use v1.5.1` if it does not.

`foundry.toml` pins solc 0.8.30, and Foundry downloads it on the first build. Install Slither 0.11.6 with `pipx`. If your default Python is too new for its dependencies, point `pipx` at Python 3.12:

```sh
pipx install slither-analyzer==0.11.6
# or: pipx install --python python3.12 slither-analyzer==0.11.6
```

Run the checks from `celo/`:

```sh
forge fmt --check
forge build
forge test
forge coverage
slither . --config-file slither.config.json --fail-low
```

Compiler warnings fail `forge build`.

Slither ignores only vendored code under `celo/lib/`. Tests and scripts are left out because the build it runs skips them, not because of a path filter. Any vulnerable contract under `celo/src/`, in any subfolder, must make the slither command fail. If it passes, the config is hiding code.

## License

Apache 2.0. See [LICENSE](LICENSE).

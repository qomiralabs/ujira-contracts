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

### Soroban (Stellar)

The Soroban contracts were last built with Rust 1.98.1, soroban-sdk 29.0.1 and stellar-cli 28.1.0 on macOS (Apple silicon).

1. Install Rust with rustup (see https://rustup.rs):

   ```sh
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   ```

2. Install the pinned toolchain and the contract target. `soroban/rust-toolchain.toml` pins Rust 1.98.1 with `rustfmt`, `clippy` and the `wasm32v1-none` target:

   ```sh
   rustup toolchain install 1.98.1 --component rustfmt --component clippy
   rustup target add wasm32v1-none --toolchain 1.98.1
   ```

   The pin only applies when cargo runs inside `soroban/`. If you run cargo from the repository root with `--manifest-path soroban/Cargo.toml`, rustup uses your default toolchain instead, so add the target to that one as well:

   ```sh
   rustup target add wasm32v1-none
   ```

3. Install stellar-cli 28.1.0, with either of:

   ```sh
   brew install stellar-cli
   cargo install --locked stellar-cli --version 28.1.0
   ```

   Homebrew installs whatever version it currently ships, so check that `stellar --version` prints `stellar 28.1.0`. If it prints anything else, use the pinned `cargo install` line instead.

4. Check, test and build from `soroban/`:

   ```sh
   cd soroban
   cargo fmt --all -- --check
   cargo clippy --all-targets -- -D warnings
   cargo test
   stellar contract build
   ```

   The contract wasm lands in `soroban/target/wasm32v1-none/release/`. Build it with `stellar contract build`, not `cargo build --target wasm32v1-none`. Since soroban-sdk 28 the SDK refuses a plain cargo build for wasm, because the contract spec embedded in the wasm is only correct after stellar-cli has trimmed it.

### Celo (Foundry)

Install Foundry 1.5.1 with `foundryup`, then fetch the pinned `forge-std` submodule:

```sh
curl -L https://foundry.paradigm.xyz | bash
foundryup --install v1.5.1
git submodule update --init --recursive
```

Check that `forge --version` prints `1.5.1`. A newer local Foundry can format, lint or compile differently from the pinned one, so switch back with `foundryup --use v1.5.1` if it does not.

`foundry.toml` pins solc 0.8.37 and the Prague EVM, and Foundry downloads the compiler on the first build. Install Slither 0.11.6 with `pipx`. If your default Python is too new for its dependencies, point `pipx` at Python 3.12:

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

From the repository root, check bytecode sizes (needs `jq`) and run its self test:

```sh
scripts/check-sizes.sh
scripts/check-sizes.test.sh
```

A contract whose name ends in `Factory` may use up to 61,440 bytes of runtime code, Celo's 65,536 byte limit less a 4 KiB margin. Every other contract, the mandate included, must stay within 24,576 bytes. Initcode must stay within Celo's 131,072 byte limit. Name a contract `...Factory` only if it deploys mandates.

Slither ignores only vendored code under `celo/lib/`. Tests and scripts are left out because the build it runs skips them, not because of a path filter. Any vulnerable contract under `celo/src/`, in any subfolder, must make the slither command fail. If it passes, the config is hiding code.

## License

Apache 2.0. See [LICENSE](LICENSE).

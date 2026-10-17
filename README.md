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

## Invisible characters

Literal bidirectional controls and invisible characters can make code read differently from how it runs. To check the tree:

```sh
scripts/check-invisible-characters.py
```

It reads every tracked file as bytes. Only files whose extension is on the short binary list in the script (`png`, `jpg`, `jpeg`, `gif`, `webp`, `ico`, `woff`, `woff2`, `wasm`, `pdf`) are skipped, and only when their content starts with that format's signature; a listed name with other content fails. Every other file must be valid UTF-8 and is checked in full, and a NUL byte is reported as U+0000, so a NUL cannot make a source file skip the check. It fails on every character in the Unicode categories Cf, Zl and Zp, such as the byte order mark, zero width characters, the bidirectional controls U+202A to U+202E and U+2066 to U+2069, and U+2028 and U+2029, naming the file, line, column and code point. Categories come from Python's `unicodedata` tables, and the script uses the standard library only. It also fails on the no break space (U+00A0), narrow no break space (U+202F) and minus sign (U+2212), except in files listed in `scripts/invisible-characters-allowlist.txt`. Write any of these characters as an escape, such as `\u{202e}` in Rust or `unicode"\u202e"` in Solidity. `scripts/check-invisible-characters.test.sh` tests the check itself.

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

   Check dependencies against `soroban/deny.toml` (RustSec advisories, permissive licenses only, crates.io only) with cargo-deny 0.20.2:

   ```sh
   cargo install --locked cargo-deny --version 0.20.2
   cargo deny --locked check advisories licenses sources
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

### Foundry environment

Any `FOUNDRY_*` or `DAPP_*` environment variable overrides `foundry.toml`, and forge quietly falls back to the default profile when `FOUNDRY_PROFILE` names one that does not exist. CI runs every forge command through `scripts/foundry-env.sh`, which clears those variables and refuses an unknown profile. To run a check the same way locally:

```sh
scripts/foundry-env.sh -- forge build
scripts/foundry-env.sh --profile default -- scripts/check-sizes.sh
```

`scripts/foundry-env.test.sh` tests the wrapper.

Slither is installed in CI from `celo/slither-requirements.txt`, which pins Slither 0.11.6 and every dependency by hash.

## Continuous integration

`.github/workflows/ci.yml` runs on every pull request and on pushes to `development`, `staging` and `main`, with read-only permissions and no secrets. A newer push to the same branch cancels the run in progress.

| Job | What it checks |
| --- | --- |
| `headers` | actionlint on the workflows, the license header check, the invisible character check and their self tests. |
| `soroban` | From `soroban/`, with only the `rust-toolchain.toml` toolchain: `cargo fmt`, `cargo deny`, `cargo clippy` with warnings denied, `cargo test`, unchanged test snapshots, `stellar contract build`, and a clean tree afterwards. |
| `celo` | From `celo/`, through `scripts/foundry-env.sh`: `forge fmt --check`, `forge build` with warnings denied, `forge test`, `forge coverage`, Slither with `--fail-low`, the size check (every compiled `src/` contract must be checked), both self tests, and a clean tree afterwards. |

Every action is pinned to a full commit SHA. actionlint, stellar-cli, cargo-deny and Foundry are release binaries checked against a SHA256 in the workflow; bump a version and its hash together.

## License

Apache 2.0. See [LICENSE](LICENSE).

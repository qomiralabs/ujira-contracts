# ujira-contracts

UJIRA mandate contracts for Stellar (Soroban), built from a chain-independent spec.

## Repository layout

| Folder | What it holds |
| --- | --- |
| `spec/` | The mandate spec, independent of chain and language, and the shared JSON test vectors. |
| `soroban/` | The Stellar mandate contract (Rust, Soroban SDK). |
| `spikes/` | Write-ups and fixtures of time-boxed experiments. The prototypes live under `soroban/spikes/` and never ship. |
| `tests/` | Behavior tests that drive the contract from the spec's vectors. |
| `exploits/` | Adversarial exploit tests against the contract. |
| `audits/` | Internal and external audit reports. |
| `scripts/` | Repository checks, such as the license header check. |

## License headers

Every Rust source file starts with these two lines:

```text
// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 Qomira Labs
```

The SPDX line comes first, so a license scanner finds it without parsing the file. Use the year the file was created. To check the tree:

```sh
scripts/check-headers.sh
```

It names every tracked `.rs` file that is missing the header. Cargo build output under `target/` is skipped. `scripts/check-headers.test.sh` tests the check itself.

## Invisible characters

Literal bidirectional controls and invisible characters can make code read differently from how it runs. To check the tree:

```sh
scripts/check-invisible-characters.py
```

It reads every tracked file as bytes. Only files whose extension is on the short binary list in the script (`png`, `jpg`, `jpeg`, `gif`, `webp`, `ico`, `woff`, `woff2`, `wasm`, `pdf`) are skipped, and only when their content starts with that format's signature; a listed name with other content fails. Every other file must be valid UTF-8 and is checked in full, and a NUL byte is reported as U+0000, so a NUL cannot make a source file skip the check. It fails on every character in the Unicode categories Cf, Zl and Zp, such as the byte order mark, zero width characters, the bidirectional controls U+202A to U+202E and U+2066 to U+2069, and U+2028 and U+2029, naming the file, line, column and code point. Categories come from Python's `unicodedata` tables, and the script uses the standard library only. It also fails on the no break space (U+00A0), narrow no break space (U+202F) and minus sign (U+2212), except in files listed in `scripts/invisible-characters-allowlist.txt`. Write any of these characters as an escape, such as `\u{202e}` in Rust. `scripts/check-invisible-characters.test.sh` tests the check itself.

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

   The contract wasm lands in `soroban/target/wasm32v1-none/release/`. Build it with `stellar contract build`, not `cargo build --target wasm32v1-none`. A plain `stellar contract build` also builds any spike crate in the workspace; to build only the production contracts (the cdylib crates under `soroban/contracts/`) as CI does, run `scripts/soroban-build.sh`. It builds each one with `--package` and fails unless the release folder then holds exactly their wasm. `scripts/soroban-build.test.sh` tests it. Since soroban-sdk 28 the SDK refuses a plain cargo build for wasm, because the contract spec embedded in the wasm is only correct after stellar-cli has trimmed it.

## Continuous integration

`.github/workflows/ci.yml` runs on every pull request and on pushes to `development`, `staging` and `main`, with read-only permissions and no secrets. A newer push to the same branch cancels the run in progress.

| Job | What it checks |
| --- | --- |
| `headers` | actionlint on the workflows, the license header check, the invisible character check, the mandate spec rule ID check and their self tests. |
| `fixtures` | Rebuilds the attestation parity fixture from its generator under `spikes/attestation-parity/generator/` and fails unless it matches the committed file byte for byte. |
| `soroban` | From `soroban/`, with only the `rust-toolchain.toml` toolchain: `cargo fmt`, `cargo deny`, `cargo clippy` with warnings denied, `cargo test`, unchanged test snapshots, the build script self test, `scripts/soroban-build.sh` (production contracts only), a clean tree afterwards, and the spike wasm fixtures rebuilt from source byte for byte. |

Every action is pinned to a full commit SHA. actionlint, stellar-cli and cargo-deny are release binaries checked against a SHA256 in the workflow; bump a version and its hash together.

## License

Apache 2.0. See [LICENSE](LICENSE).

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

   The contract wasm lands in `soroban/target/wasm32v1-none/release/`. Build it with `stellar contract build`, not `cargo build --target wasm32v1-none`. A plain `stellar contract build` also builds any spike crate in the workspace; to build only the production contracts (the cdylib crates under `soroban/contracts/`) as CI does, run `scripts/soroban-build.sh`. It builds each one with `--package` and fails unless the release folder then holds exactly their wasm. `scripts/soroban-build.test.sh` tests it. Since soroban-sdk 28 the SDK refuses a plain cargo build for wasm, because the contract spec embedded in the wasm is only correct after stellar-cli has trimmed it.

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

A contract listed in `celo/factory-allowlist.txt` may use up to 61,440 bytes of runtime code, Celo's 65,536 byte limit less a 4 KiB margin. Entries name the source path and the contract, as `src/<file>.sol:<Contract>`; the contract's name plays no part, so a mandate renamed `...Factory` is still held to the mandate budget. Every other contract, the mandate included, must stay within 24,576 bytes. Initcode must stay within Celo's 131,072 byte limit. The check fails on an allowlist entry that is malformed, listed twice or matches no contract. Add a contract to the allowlist only if it deploys mandates, and only through a reviewed change.

Only contracts in `celo/src/` are sized, plus every concrete vendored contract under `celo/lib/` that `src/` compiles, whether `src/` deploys it, calls it or only imports it. Those are held to the mandate budget. Contracts under `celo/test/` and `celo/script/` are never sized. An allowlist containing a NUL byte or any other control character (anything but tab, line feed and carriage return) is rejected.

Slither ignores only vendored code under `celo/lib/`. Tests and scripts are left out because the build it runs skips them, not because of a path filter. Any vulnerable contract under `celo/src/`, in any subfolder, must make the slither command fail. If it passes, the config is hiding code.

### Foundry environment

Any `FOUNDRY_*` or `DAPP_*` environment variable, in any case, overrides `foundry.toml`, and forge quietly falls back to the default profile when `FOUNDRY_PROFILE` names one that does not exist. CI runs every forge command through `scripts/foundry-env.sh`, which clears those variables and refuses an unknown profile.

forge also reads some overrides itself, after the wrapper has handed over, so the wrapper cannot clear them. It refuses to run instead while any of these exists:

- a `.env` in the working directory, in `celo/`, or in any parent of either
- `~/.foundry/foundry.toml`
- `FOUNDRY_CONFIG`, in any case
- a `foundry.toml` in the working directory other than the one it checks
- an inherited `FOUNDRY_` or `DAPP_` variable whose name is not a valid shell name

That covers what forge 1.5.1 reads. A command that changes directory, or a later forge that reads a new source, can still get around it, so CI also checks the results: `forge config` must show the pinned settings and no test filter, and `forge test` must report at least one test, all passing.

To run a check the same way locally:

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
| `soroban` | From `soroban/`, with only the `rust-toolchain.toml` toolchain: `cargo fmt`, `cargo deny`, `cargo clippy` with warnings denied, `cargo test`, unchanged test snapshots, the build script self test, `scripts/soroban-build.sh` (production contracts only), and a clean tree afterwards. |
| `celo` | From `celo/`, through `scripts/foundry-env.sh`: `forge fmt --check`, `forge build --force` with warnings denied, `forge test` (at least one test must run), `forge coverage`, Slither with `--fail-low`, the size check (every compiled `src/` contract must be checked), both self tests, and a clean tree afterwards. |
| `secrets` | gitleaks over the full history with `.gitleaks.toml`, and the scan's self test. See Secret scanning below. |

Every action is pinned to a full commit SHA. actionlint, stellar-cli, cargo-deny, gitleaks and Foundry are release binaries checked against a SHA256 in the workflow; bump a version and its hash together.

## Secret scanning

The `secrets` job in `.github/workflows/ci.yml` runs `scripts/secret-scan.sh` with gitleaks 8.30.1 (MIT licensed), downloaded from its GitHub release and checked against a pinned SHA-256. It checks out the full history and scans every commit reachable from the checked out commit, not only the change, so a key that was committed and later deleted still fails. Merge commits are scanned against each parent, so a key added while resolving a merge is found, and every file is read as text, so a `.gitattributes` entry such as `-diff` or `binary` cannot hide one. The scan refuses a shallow clone and fails unless gitleaks reports scanning exactly the commits it should: every commit with a change in a file it does not delete (commits that only delete, rename or change a mode have nothing to scan). Findings are redacted in the log.

Two limits apply to any line based scanner. Binary files are handed to gitleaks as text, so a key stored as plain text inside one is found, but a compressed or encoded one is not; a commit that only adds binary files still passes. A token split across lines, or assembled from pieces in code, is not caught either. To cover the first, the scan also refuses, by name and in any case, any commit in the history that adds or changes a key or env file, binary or not: `.pem`, `.key`, `.p12`, `.pfx`, `.jks`, `.keystore`, `.ppk`, `.p8` and `.der` files, private ssh keys such as `id_rsa` or `id_ed25519` (their `.pub` halves are allowed), and `.env`, `.env.*`, `.envrc` and `*.env` files other than `.env.example`. Names are read unquoted, so non-ASCII names are checked too. Key material and environment values belong in the secret manager.

`.gitleaks.toml` holds the default rules of gitleaks 8.30.1 with upstream's global allowlist removed, so lockfiles, `node_modules`, images, `.pem`, `.key` and `.log` files, `go.sum` and files named `gitleaks.toml` are scanned like any other file. Upstream rule allowlists that list sample keys or passwords (for the GCP API key and NuGet password rules, and one hex stopword) are removed too. The rule allowlists that remain only drop placeholders, words and template syntax, and the self test scans the file with every allowlist stripped so no key shaped literal can return. Its header names the upstream file, its hash and every edit. Inline `gitleaks:allow` comments are ignored and any `.gitleaksignore` fails the scan, so `.gitleaks.toml` is the only allowlist. Each allowlist entry must cover a single synthetic test fixture and say why it is safe; path wide and rule wide exclusions are not allowed. The one entry today allows the two exact public test keys on the `private_key` lines of `spikes/attestation-parity/fixture.json`, for the generic rule only. Both are keccak256 of public `TEST-ONLY` labels named in the file. Any other value on those lines, any other finding in that file and the same values in any other file still fail.

On pull requests the scan reads `.gitleaks.toml` from the base branch rather than from the change, so an edit to that file in a pull request does not change the rules that pull request is scanned with; the edit takes effect once it is merged. This is not a complete guarantee: a pull request can still edit `scripts/secret-scan.sh` or the workflow, and review is the control for those. A base branch without `.gitleaks.toml` gets the gitleaks built in default rules, never an empty set, and for that run upstream's global allowlist and its path skips apply again.

`scripts/secret-scan.test.sh` plants generated fake keys in throwaway repositories and checks that each one fails: in an ordinary commit, in a commit later deleted, behind `gitleaks:allow`, behind a `.gitleaksignore`, in a merge resolution, in files marked `-diff` or `binary`, in each path upstream skips (lockfiles, `node_modules`, `.svg`, a text `.png`, `.pem`, `.key`, `.log`, `go.sum`, any `gitleaks.toml`), and in a pull request that tries to replace the rules. It checks that key and env files are refused, also when removed later and with non-ASCII names, that `.pub` keys and `.env.example` pass, that delete only, rename only, mode only, binary, symlink and empty commits pass with the commit count matched exactly, and that `.gitleaks.toml` itself holds no key shaped literal. Both scripts run locally once gitleaks is installed (`brew install gitleaks`):

```sh
scripts/secret-scan.sh
scripts/secret-scan.test.sh
```

A manual run of gitleaks must pass the config as an absolute path, as `scripts/secret-scan.sh` does (`--config "$PWD/.gitleaks.toml"`, not `--config .gitleaks.toml`). When a relative config path matches the path of the file inside the repository, gitleaks silently skips findings in the config file itself.

Merging: this repository must stay squash-only. A pull request branch may hold commits, such as test probes or fixups, that never reach `development` only because every merge is a squash merge, and the scan on `development` relies on that. Enabling merge commits or rebase merging requires revisiting the secret scan first.

Keep `.env` files untracked; only `.env.example` belongs in git.

## License

Apache 2.0. See [LICENSE](LICENSE).

# Audit coverage of the OpenZeppelin smart account

Which audits cover the OpenZeppelin `stellar-accounts` smart account that passkey wallets will run, what those audits found in the files we use, and which version to pin. This is desk research against the upstream repository at commit `ddc327e` and its eight reports, read in full where they touch the files we use. The wallet contract itself was already chosen and is not reopened here.

## Result

**Pin tag `v0.8.0`, commit `ddc327eb92d63709436da3a8455f641c2d5476c5`.** It is the latest release, and the latest audited release that includes the accounts package. No critical or high finding is open in the files we use.

**The wasm hash is not pinned yet.** The CI job `smart-account` builds it from the tag. It is written here, and into `scripts/build-smart-account.sh`, only once two separate CI runs agree. See [The pinned build](#the-pinned-build).

**Decided by the contract owner on 2026-11-02: the coverage is enough, and `v0.8.0` is pinned.** The coverage has five limits, kept under [Limits accepted](#limits-accepted) as the record of what was decided. The sharpest two: the deployable account contract is an example that no audit report lists in scope, and every report comes from OpenZeppelin auditing its own library.

## Coverage per report

Every report is by OpenZeppelin and sits in the upstream `audits/` folder. The links are pinned to the tagged commit, and the hash is the SHA-256 of the PDF. The report dates are the ones printed on the covers. The fieldwork dates are from each summary.

| Report | Library version | Auditor | Report date, fieldwork | Commits | `packages/accounts` | `verifiers/webauthn.rs` | `policies/weighted_threshold.rs` |
| --- | --- | --- | --- | --- | --- | --- | --- |
| [0.1.0-RC](https://github.com/OpenZeppelin/stellar-contracts/blob/ddc327eb92d63709436da3a8455f641c2d5476c5/audits/Stellar%20Contracts%20Library%200.1.0-RC%20Audit.pdf) | 0.1.0-rc | OpenZeppelin | 2025-02-20, 2025-02-03 to 02-07 | at `01dbcb5` | No, the package did not exist | No | No |
| [v0.2.0](https://github.com/OpenZeppelin/stellar-contracts/blob/ddc327eb92d63709436da3a8455f641c2d5476c5/audits/Stellar%20Contracts%20Library%20v0.2.0%20Audit.pdf) | 0.2.0-rc | OpenZeppelin | 2025-07-03, 2025-03-31 to 04-09 | `ca8da91` to `fc03869` | No | No | No |
| [v0.3.0-rc.2](https://github.com/OpenZeppelin/stellar-contracts/blob/ddc327eb92d63709436da3a8455f641c2d5476c5/audits/Stellar%20Contracts%20Library%20v0.3.0-rc.2%20Audit.pdf) | 0.3.0-rc.2 | OpenZeppelin | 2025-07-08, 2025-06-04 to 06-18 | `d3741c3` to `cf05a5d` | No | No | No |
| [v0.5.0](https://github.com/OpenZeppelin/stellar-contracts/blob/ddc327eb92d63709436da3a8455f641c2d5476c5/audits/Stellar%20Contracts%20Library%20v0.5.0%20Audit.pdf) | 0.5.0-rc.2 | OpenZeppelin | 2025-10-28, 2025-09-09 to 09-22 | `857bd36` to `028c429` | Yes, in full: the package is new in this range, 0 files at the base | Yes, in full | Yes, in full |
| [v0.5.0 re-audit](https://github.com/OpenZeppelin/stellar-contracts/blob/ddc327eb92d63709436da3a8455f641c2d5476c5/audits/Stellar%20Contracts%20Library%20v0.5.0%20Re-Audit.pdf) | 0.5.0-rc.3 | OpenZeppelin | 2025-10-23, 2025-10-07 to 10-14 | `028c429` to `7b14668` | Yes, the diff | Yes, the diff | Yes, the diff |
| [v0.6.0](https://github.com/OpenZeppelin/stellar-contracts/blob/ddc327eb92d63709436da3a8455f641c2d5476c5/audits/Stellar%20Contracts%20Library%20v0.6.0%20Audit.pdf) | 0.6.0-rc.2 | OpenZeppelin | 2026-01-07, 2025-12-04 to 12-24 | `a5fada3` to `0a89f54` | Yes, the diff | Yes, the diff | Yes, the diff |
| [v0.7.0](https://github.com/OpenZeppelin/stellar-contracts/blob/ddc327eb92d63709436da3a8455f641c2d5476c5/audits/Stellar%20Contracts%20Library%20v0.7.0%20Audit.pdf) | 0.7.0-rc.1 | OpenZeppelin | 2026-04-06, 2026-03-02 to 03-19 | `82f9711` to `239a2a7` | Yes, the diff (the largest, a rework of authorization) | Yes, the diff | Yes, the diff |
| [v0.8.0](https://github.com/OpenZeppelin/stellar-contracts/blob/ddc327eb92d63709436da3a8455f641c2d5476c5/audits/Stellar%20Contracts%20Library%20v0.8.0%20Audit.pdf) | Two snapshots before the 0.8.0 tag | OpenZeppelin | 2026-10-20, 2026-09-14 to 09-29 | `fbfde38` to `a5bd8cb`, and `9c5e279` to `2128f11` | Yes, the diffs, for `smart_account/` and `lib.rs` | Yes, range one lists the file | No change in either range and not listed, last reviewed in v0.7.0 |

PDF hashes, for anyone comparing their copy:

| Report | SHA-256 |
| --- | --- |
| 0.1.0-RC | `c97fb5c9dcdf549ddd16de3d944927c55800be19e30fe109b03025587a74c907` |
| v0.2.0 | `59fa25010851fbcad18cd69f2a4d2cedd0e03b23fa720c150f64fc355e6d336d` |
| v0.3.0-rc.2 | `7e096e3692e61485080995031e1f2dca056433001062b29d988f0e119a563daa` |
| v0.5.0 | `5e9949eddbcdd38f1c7651f405c5205d7c1af90ff1bec21d35a5607f1b14a600` |
| v0.5.0 re-audit | `8f89a6a71ff1c3a9c4530f58e0b0a17d45ab8b1fe2779b887c8bd3a6725c6451` |
| v0.6.0 | `769b01596bc2ed0e7f00f4bd605c59c4df83fda6cffd045eb7c8fb181d8bf2af` |
| v0.7.0 | `9514b16815e853d656e0f284fb8d82b98135c04b394595f3c3f97e68804f40f5` |
| v0.8.0 | `52cff3150ad82eea6c62d4ff564c79149952b98c17d1cddc05fb2964fe5764d0` |

### Does the chain of diffs reach the tag?

All reports from v0.5.0 on are diff audits, so coverage holds only if the ranges join up. Measured with `git diff` over the files we use (`smart_account/`, `verifiers/webauthn.rs`, `verifiers/mod.rs`, `verifiers/utils/`, `policies/weighted_threshold.rs`, `policies/mod.rs`, `lib.rs`), counting changed lines that are not blank and not comments:

| Range | Changed code lines | What it is |
| --- | --- | --- |
| `857bd36` to `028c429` | the whole package | Audited, v0.5.0 |
| `028c429` to `7b14668` | 204 | Audited, re-audit |
| `7b14668` to `a5fada3` | 67 | The re-audit's fix commits (#443, #452, #454) and a signer rename (#469). The report records the fixes, but no audit range lists these commits |
| `a5fada3` to `0a89f54` | 49 | Audited, v0.6.0 |
| `82f9711` to `239a2a7` | 821 | Audited, v0.7.0 |
| `239a2a7` to `d55dd37` | 393 | The v0.7.0 fix commits, which the report says are all merged at `d55dd37` |
| `d55dd37` to `9c5e279` | 5 | An import list only |
| `9c5e279` to `2128f11` | 46 | Audited, v0.8.0, the digest change (#868) |
| `fbfde38` to `a5bd8cb` | 0 | Audited, v0.8.0, comments only in these files |
| **`2128f11` to `v0.8.0`** | **61** | **After the audit snapshots, no reported review** |

Both audited heads of the v0.8.0 report are ancestors of the tag, checked with `git merge-base --is-ancestor`. The report's own conclusion calls the ranges "the 0.8.1 line on main" and "the v0.9.0 branch", names that no tag or branch carries, so ancestry is the proof used here and not the names.

The 61 lines are two commits:

- `8e25220` (40 lines), the fix for advisory [GHSA-jpg9-m7w4-c7wc](https://github.com/OpenZeppelin/stellar-contracts/security/advisories/GHSA-jpg9-m7w4-c7wc), which is finding CR-01 below.
- `b40c5ea` (21 lines), the bump to soroban-sdk 28, which adds the `CreateContractExternalRef` rule type and matches on it.

## Why v0.8.0 and not v0.7.3

| | `v0.8.0` | `v0.7.3` |
| --- | --- | --- |
| Released | 2026-10-20, latest | 2026-10-20, a security backport |
| Advisory fix | Yes | Yes |
| Audited heads are ancestors | Yes | No, the v0.8.0 ranges are not in it |
| Signers commit to the account address | Yes, `AuthDigestPreimage { account, signature_payload, context_rule_ids }` | No, `sha256(signature_payload || context_rule_ids)` only |
| soroban-sdk | 28.0.0 | 26.1.0 |

Binding the account address means a signature made for one wallet cannot be reused by another wallet that shares the same signer, which matters because a passkey is a natural thing to share between two wallets of one person. The SDK version does not constrain us: the wallet is a separate wasm built from upstream's own lockfile, and our workspace on soroban-sdk 29 only calls it.

## Every finding in the files we use

Findings against `smart_account/`, `verifiers/webauthn.rs`, the shared verifier code and `policies/weighted_threshold.rs`. Findings against `spending_limit.rs`, `simple_threshold.rs` and `ed25519.rs` alone, and against other packages, are left out because passkey wallets do not use those files. A finding about policies in general is kept.

Status is as the report states it. "Checked" means this research also read the code at the tag.

| Report | ID, severity | Finding | Status |
| --- | --- | --- | --- |
| v0.5.0 | C-03, critical | Signer aliasing: verifiers truncated over-long keys, so one key could pose as several signers and meet a threshold alone | Resolved, #384 |
| v0.5.0 | H-03, high | A rule left with no signers and no policies became allow-all | Resolved, #409 |
| v0.5.0 | M-05, medium | A policy is not told when a rule's signers change, so a threshold can become unreachable or silently weaker | **Partially resolved**, #412, by documentation. Open by design, see below |
| v0.5.0 | M-08, medium | Unlimited and duplicate rules could exhaust resources | Resolved, #427. Checked: since v0.7.0 the caller names the rule for each context, so authorization no longer scans the list |
| v0.5.0 | L-05, low | `extract_from_bytes` accepted a short range and then panicked | Resolved, #407 |
| v0.5.0 | L-06, low | Superfluous valid signatures are accepted, unlike native Stellar | **Acknowledged, not resolved**, by design |
| Re-audit | H-01, high | The rule ID counter ran out and blocked new rules forever | Resolved, #452 |
| Re-audit | N-02, note | Misleading comments, one in the accounts package | Resolved, #443 |
| v0.6.0 | N-02, note | Rule fingerprint left out the expiry, against its documentation | Resolved, #540; fingerprints were later removed |
| v0.7.0 | H-01, high | `context_rule_ids` were not signed, so a sponsor could swap in a weaker rule after signatures were collected | Resolved, #655. Checked: the digest now binds the rule IDs and the account |
| v0.7.0 | M-05, medium | Signer keys are stored raw, so a key in another encoding verifies but does not match | **Acknowledged, not resolved**, a deliberate choice so the passkey credential ID stays on chain |
| v0.7.0 | M-06, medium | `do_check_auth` called attacker-chosen verifiers before filtering signers | Resolved, #656 and #667. Checked: unlisted signers are refused before `authenticate` |
| v0.7.0 | L-05, low | No view from a signer or policy to its ID | Resolved, #664 |
| v0.7.0 | L-09, low | Repeated canonicalization calls | Resolved, #657 |
| v0.7.0 | N-01, N-02, note | Documentation corrections, some in these files | Resolved, #658 |
| v0.7.0 | N-04, N-05, note | `canonicalize_key` requirements and a missing batch helper | Resolved, #657 |
| v0.8.0 | L-02, low | Misleading documentation, including two ways an account can lock itself out for good | Resolved, #932. Checked: commit `6127315` is in the tag |
| v0.8.0 | CR-01, client reported | A `Delegated` signer passes with no keys when the delegate is the direct caller of `__check_auth`, so anyone could drain a wallet set up that way | **See below.** Checked: fixed in code at the tag |

No high or critical finding is open.

### CR-01, the delegated signer flaw

The report is inconsistent about this one, so it is recorded exactly. CR-01 has no `Update:` line, unlike CR-02 and CR-03. The header says 3 of 3 client reported issues are resolved, but also says 9 of 12 in total, while its own rows add to 10.

The code answers the question. The fix is in the tag, in `do_check_auth`, `validate_no_delegated_context_contract` and `validate_no_self_delegation`, and the public advisory lists `v0.8.0` as patched and rates it medium. It landed in `8e25220` on 2026-10-20, after the audit snapshots, and has regression tests, but no report says an auditor reviewed it.

The advisory also says the check runs in the account that authorizes back, so an account is protected only when the accounts it delegates to are also patched. A passkey wallet avoids the whole class by using `External` signers only.

### Open upstream issues in these files

These are not audit findings. They are open on the upstream tracker as of this research.

- [#891](https://github.com/OpenZeppelin/stellar-contracts/issues/891), [#892](https://github.com/OpenZeppelin/stellar-contracts/issues/892): `weighted_threshold::install` totals every entry of `signer_weights`, including addresses that are not signers of the rule, so a rule can be installed with a threshold it can never reach. This costs availability, not funds, and only a principal's own setup can cause it. Our create flow must check that every weighted address is a rule signer.
- [#852](https://github.com/OpenZeppelin/stellar-contracts/issues/852): the threshold policies put the whole authorization `Context` in their event, so a call with very large arguments can pass the policy and then fail on the network's event size limit. Mandate calls carry bounded arguments, and the largest should be measured against the limit in the lifecycle vectors.

## Limits accepted

The issue asks for the latest audited release with no open high or critical finding in the files we use, and `v0.8.0` is that. The contract owner accepted these limits on 2026-11-02. Nothing here is on mainnet, and the contracts need an external audit before they are, so items 1 and 2 belong in that audit's scope.

1. **The deployable account is not in any audit scope.** Every report lists `packages/` files only. What we deploy is `examples/multisig-smart-account/account`, about 100 lines that wire the audited library into a contract. The WebAuthn verifier and the weighted threshold policy are likewise thin examples. No report lists a deployable wasm, so the choice is this example or a wrapper of our own, and a wrapper of our own would be just as unaudited and would not be reproducible from the upstream tag alone. The example was chosen for that reason.
2. **61 changed lines reached the tag after the last audited snapshot.** They include the security fix for CR-01, as above.
3. **The auditor is not independent.** Every report is OpenZeppelin reviewing OpenZeppelin's own library, and no third party report exists in the upstream folder.
4. **The reports disclaim reliance.** Each cover says it "has been prepared solely for the named client and it may not be relied upon by any third parties". We are a third party. This may matter for how the audits are described to customers, and that is a legal question, not an engineering one.
5. **The account can upgrade itself.** The example has `upgrade`, gated on the account's own authorization, so UJIRA cannot use it but the wallet's signers can move off the pinned code. `CHAINOPS-CREATE-006` reads the code hash for this reason, after every change the operator relays. A wallet that upgrades outside the operator is only caught if operations re-read the hash before relying on it, and that rule does not yet say when.

## What this means for the work that follows

Facts from the reports and the code that the next issues must respect.

- **A `CallContract` rule covers the whole contract.** The v0.7.0 report warns that such a rule ignores the function name and arguments, and threshold policies do not inspect them. A `mandate` rule therefore authorizes every principal call on that mandate, `update_limits`, `add_role` and `propose_principal` included, not just `approve`. Its threshold must be treated as control of the mandate, and `CHAINOPS-CREATE-002` and `CHAINOPS-CREATE-006` do not say so yet.
- **Policy thresholds do not follow signer changes.** Adding a signer to an N-of-N rule silently makes it N-of-(N+1), and removing one can make the rule unreachable (M-05 above). `CHAINOPS-CREATE-006` re-reads the wallet after every relayed change, and that read must include both.
- **An account can lock itself out for good.** Removing the last rule, or letting the last rule's `valid_until` lapse, leaves nothing that can authorize a new one (v0.8.0 trust assumptions). Anything we relay must refuse such a change.
- **Verifiers and policies are trusted code.** The report assumes verifier contracts behave correctly, and a signer names its verifier by address. The allowed list in `CHAINOPS-CREATE-006` names wallet code hashes only, so it must also cover the verifier and the policy. The build below produces all three.
- **Origin and RP ID are not checked on chain.** `webauthn.rs` documents that it omits both, and leaves them to the authenticator and the platform. It does require the user present and user verified flags, so a passkey without user verification cannot sign. The challenge binds the signature to the digest. Phishing protection therefore rests on the browser and the passkey's RP ID, and the wallet UI must set them correctly.
- **Nothing extends the account's instance TTL.** The v0.7.0 report leaves this to the integrator, and the example does not do it. An idle wallet archives and must be restored, which is a cost for the sponsorship budget (qomiralabs/ujira#74) and not a safety problem.
- **The constructor makes one rule, and it is not the rule the spec names.** The example's constructor takes `signers` and `policies` and creates a single `Default` rule named `multisig`. `CHAINOPS-CREATE-002` has its reader load an `admin` rule and a `mandate` rule, and neither exists on a new wallet from this code. The `Default` rule covers every call and can serve as the admin rule, but under another name. A `mandate` rule cannot be set at creation at all, because the mandate's address derives from the wallet's, and the wallet's address would then have to hash its own mandate's. The wallet's signers must add it afterwards, and that is one more signature in onboarding. The contract owner decided on 2026-11-02 to accept that shape and not to write a wrapper: a wallet is created with its `Default` rule only, and its signers add the `mandate` rule once the mandate exists. The off-chain rules are changed to match in their own pull request.

## The pinned build

`scripts/build-smart-account.sh <tag>` checks the tag against the commit pinned in the script, fetches that commit into a fresh temporary directory, and builds it with `stellar contract build --locked`, using upstream's own `Cargo.lock`. The Rust toolchain is the one `soroban/rust-toolchain.toml` pins, and stellar-cli must be the version CI pins, because both are recorded in the wasm's metadata.

| Input | Value |
| --- | --- |
| Tag | `v0.8.0` |
| Commit | `ddc327eb92d63709436da3a8455f641c2d5476c5` |
| Rust | 1.98.1, as `soroban/rust-toolchain.toml` pins |
| stellar-cli | 28.1.0, as CI pins |
| Target | `wasm32v1-none` |

| Package, from `examples/multisig-smart-account/` | Wasm SHA-256 |
| --- | --- |
| `multisig-account-example` (the smart account) | Not measured |
| `multisig-webauthn-verifier-example` | Not measured |
| `multisig-weighted-threshold-policy-example` | Not measured |

To check it:

```sh
scripts/build-smart-account.sh v0.8.0        # prints the account's hash
scripts/build-smart-account.sh --all v0.8.0  # prints all three, one per line
```

Upstream attaches no wasm to the `v0.8.0` release, so there is no published hash to compare with. The file `examples/multisig-smart-account/factory/testdata/multisig_account_example.wasm` at the tag was last changed on 2026-09-10, before the tag, so it is not the tagged account and its hash must not be used.

The build compiles upstream's source and its dependencies, including their build scripts, on the machine that runs it, so it runs in the CI job `smart-account` and not on a workstation. The job prints the three hashes in its log and its summary, keeps the wasm as the artifact `smart-account-wasm-v0.8.0`, and fails if a hash differs from the one the script pins. The hashes are pinned only after two separate runs, each from a fresh checkout on a fresh runner, printed the same values. The script's refusals are tested by `scripts/build-smart-account.test.sh`.

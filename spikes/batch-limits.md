# Batch limits on Soroban

How many payouts one `request_payouts` or `execute_batch` call can carry, measured instead of guessed, and the value of `MAX_BATCH` that follows. `MAX_BATCH` is a spec constant that cannot change after mainnet without a new version.

This note is the measurement of the prototype that set the limits. `soroban/batch-limits` holds the production contracts to them in CI.

## Recommendation

| Constant | Value | Applies to |
| --- | --- | --- |
| `MAX_BATCH` | **10** items | `request_payouts`, `execute_batch`, `cancel_batch` |
| `MAX_BATCH_TRANSFERS` (new) | **18** transfers | `execute_batch`: every leg of every item, plus one per item with a non-zero fee |

A batch outside either bound fails with `BATCH_INVALID`, the same code an item count outside `[1, MAX_BATCH]` already gets.

Why two numbers and not one: on Soroban the limit that binds is the per-transaction **contract events size, 16,384 bytes**, which counts the events and the return value. Instructions, memory, entries and bytes are far from their limits. Every SAC transfer emits its own `transfer` event that the mandate cannot shrink, and every leg also appears in the mandate's own `payout_executed` event. So the cost of an execute batch follows the number of transfers, not the number of items. Measured per item:

Run 2026-11-02, base commit `4aa2be1`, prototype wasm sha256 `3094275c27ce8fe7b2fab3ddad4b9bf6e6350a17537343b0dc0b3e8591eb1a84`.

| Part of an `execute_batch` item | Event bytes |
| --- | --- |
| `payout_executed` base | 344 |
| a leg to a classic account (its SAC event plus its `legs` entry) | 352 |
| a leg to a tagged (muxed `M`) destination, which adds `to_muxed_id` and a wider `legs` entry | **416** |
| a fee transfer (the fee address is never tagged) | 240 |

Tagged legs are the worst case, so the cap is set on them. The worst plan for a transfer budget puts as many items as `MAX_BATCH` allows and makes every transfer a tagged leg (`transfer_cap_sweep`):

Run 2026-11-02, base commit `4aa2be1`, prototype wasm sha256 `3094275c27ce8fe7b2fab3ddad4b9bf6e6350a17537343b0dc0b3e8591eb1a84`.

| Transfers, 10 items, all tagged legs, no fee | Event bytes (with return value) | Share |
| --- | --- | --- |
| 16 | 10,100 | 61.6% |
| 17 | 10,516 | 64.2% |
| **18** | **10,932** | **66.7%** |
| 19 | 11,348 | 69.3% |
| 20 (the earlier proposal) | 11,764 | 71.8% |

19 is 69.3% in the test env, and the network ran about half a point above the env when that was last measured (see Network cross-checks), so it leaves no margin at all. 18 leaves 536 bytes under the 70 percent events line of 11,468 in the test env, about 1.3 tagged transfers. A single `MAX_BATCH` of 5 would fit the worst case, but it would halve throughput for the common 1-leg payout for no reason.

Headroom under the recommended limits (at least 30 percent required on every resource):

Run 2026-11-02, base commit `4aa2be1`, prototype wasm sha256 `3094275c27ce8fe7b2fab3ddad4b9bf6e6350a17537343b0dc0b3e8591eb1a84`.

| Worst case under the limits | Measured on | Binding resource | Use | Headroom |
| --- | --- | --- | --- | --- |
| `execute_batch`, 10 items, 18 tagged legs, no fee | test env | events | 10,932 of 16,384 bytes, 66.7% | 33.3% |
| `execute_batch`, 10 items, 18 untagged legs, no fee | test env | events | 9,780 bytes, 59.7% | 40.3% |
| `execute_batch`, 4 items x 4 tagged legs, 2 with a fee (18 transfers) | test env | events | 8,516 bytes, 52.0% | 48.0% |
| `execute_batch`, 9 items x (1 tagged leg + fee) | test env | events | 9,004 bytes, 55.0% | 45.0% |
| `request_payouts`, 10 items x 4 tagged legs, BOTH mode | test env | events | 10,332 bytes, 63.1% (instructions 27.5M, 6.9%) | 36.9% |

The test suite asserts that every case in the table stays at or under 70 percent of every limit (`recommended_limits_keep_headroom`). Every case is measured, including plans that use exactly `MAX_BATCH_TRANSFERS` transfers, so a protocol or SDK change that eats the margin fails CI.

## Limits compared against

Soroban, per transaction, read with `stellar network settings` on 2026-11-02. Testnet (ledger 5511789) and mainnet (ledger 65259360, through `https://mainnet.sorobanrpc.com`) agree on every per-transaction value, both on protocol 29. The pinned CLI does not report the footprint limit, so that one row is from 2026-10-17:

| Resource | Limit |
| --- | --- |
| Instructions | 400,000,000 |
| Memory | 41,943,040 bytes |
| Disk read entries / bytes | 200 / 200,000 |
| Write entries / bytes | 200 / 132,096 |
| Footprint entries | 400 |
| Contract events and return value | 16,384 bytes |
| Transaction size | 132,096 bytes |

## What was measured

A prototype of the batch path of the mandate. It keeps the real storage layout, check order, events and transfers, and leaves out what a batch does not touch: approvals, cancel, pause and most roles. The prototype is never deployed outside testnet.

- `soroban/spikes/batch-limits/proto` holds the contract. Its compiled wasm is committed as `soroban/spikes/batch-limits/fixtures/batch_limits_proto.wasm` (sha256 `3094275c27ce8fe7b2fab3ddad4b9bf6e6350a17537343b0dc0b3e8591eb1a84`; rebuild from `soroban/` with `stellar contract build --package batch-limits-proto`). `soroban/spikes/batch-limits` loads that wasm into the SDK test env, because the native build meters host functions only and misses most of the cost. The token is a real SAC. Destinations are fresh classic accounts with trustlines, one per leg, so every transfer touches a new trustline. Tagged series pay the same kind of account through a muxed `M` address with a full 64-bit id. One series uses contract addresses instead. Attestations are real ed25519 signatures, one per leg, over the 47 byte version 2 message that binds chain, mandate, recipient, country policy, policy epoch, expiry, destination and tag. Resources come from the env's cost estimate, plus the XDR size of the return value, because the network counts that against the events limit. The `tx_size` share is an estimate of the whole envelope: the arguments twice (the auth entry repeats the call), plus 100 bytes per footprint key and 400 for the rest. That is calibrated on testnet, where a 16-item 4-leg request had a 47,016 byte envelope and the estimate gives 46,976. It is never the binding resource.
- Batch sizes 1, 10, 20 and 40; 1, 2 and 4 legs; ALLOWLIST, ATTESTED and BOTH modes; a fee on every row except one 1-leg series. Every item is checked to land APPROVED and then EXECUTED.
- The suite was run twice; the numbers are identical. The run was the debug test build rather than `--release`, because the costs come from the wasm and the host's cost model and do not depend on how the test harness itself is compiled. It ran on the pull request head `5b1fc07` that became `4aa2be1`; the two differ only in the mandate's source, which the prototype does not use.

```
cd soroban && cargo test -p batch-limits-spike -- --nocapture --test-threads=1
```

## Network cross-checks

**Not re-run.** Everything in this section was measured on 2026-10-17 on earlier builds of the prototype, before it moved to ed25519, and is kept as dated history. Re-running it needs a testnet deployment. The figures that depend on the signature scheme are the instructions of the attested requests; event bytes do not change with it, and for the batches the new run also measures (18 and 20 tagged legs on 10 items, 2 legs to classic accounts on 10 items), its event bytes equal the test env column here.

Every transaction below was built from a fresh simulation with padded write bytes and judged by `getTransaction`, not by the CLI's summary.

Soroban testnet, the prototype in ATTESTED mode, protocol 29. The untagged rows ran on an earlier build of the prototype (`629cd1a0...`), which differs from the committed fixture only by a doc comment in the contract spec and an equivalent range check: instructions and memory differ by at most 0.11 percent, and entries, bytes and events are identical. The tagged rows ran on the committed fixture itself (`03132e36...`, fetched back from the network and compared). Destinations are contract addresses unless the row says otherwise:

| Call | Result | Network figure | Test env |
| --- | --- | --- | --- |
| `request_payouts` 16 items x 4 legs (tx `f0022483...`, ledger 5244059) | SUCCESS | 16,076 event bytes; 164,785,899 instructions | 16,012 bytes; about 159.9M instructions (straight line through the measured points) |
| `request_payouts` 17 items x 4 legs (tx `b8aa14f5...`, ledger 5244062) | FAILED `resource_limit_exceeded`: "return value pushes events size above network config maximum", 17,080 > 16,384 | | predicted over: 17,012 |
| `execute_batch` 9 items x (4 legs + fee) (tx `bf500256...`, ledger 5244069) | FAILED `resource_limit_exceeded`: "total events size exceeds network config maximum" | 16,616 when it stopped | predicted over: 17,644 |
| `execute_batch` 8 items x (4 legs + fee) (tx `34661070...`, ledger 5244072) | SUCCESS | 15,844 event bytes; 12,678,765 instructions | 15,684 bytes |
| `execute_batch` 10 items x 2 legs to classic accounts (tx `149001a6...`, ledger 5244218) | SUCCESS | 10,564 event bytes; 42 writes; 2,320 read bytes | 10,484 bytes; 43 writes; 2,320 read bytes |
| `execute_batch` 10 items, 18 tagged legs (8 x 2 + 2 x 1), no fee (tx `c5fa1fd2...`, ledger 5248153) | SUCCESS | 11,004 event bytes (67.2%); 40 writes; 7,082,638 instructions | 10,932 bytes (66.7%) |
| `execute_batch` 10 items x 2 tagged legs, no fee (tx `3c52d0d9...`, ledger 5248162) | SUCCESS | 11,844 event bytes (72.3%) | 11,764 bytes (71.8%) |

The 17-item request only fails if the return value counts. It does, and the test env model, which adds it, predicts that failure. The env is within 1 percent of the network on event bytes and within 3 percent on instructions; simulation declares about 4 percent more instructions than are used.

## Findings beyond the limit

Run 2026-11-02, base commit `4aa2be1`, prototype wasm sha256 `3094275c27ce8fe7b2fab3ddad4b9bf6e6350a17537343b0dc0b3e8591eb1a84`.

History: on 2026-10-17 the prototype verified attestations with secp256k1 at about 10.05M instructions per 4-leg item, almost all signature recovery.

- The return value of `request_payouts` (an `ItemResult` per item, about 116 bytes each) is charged against the same 16,384 bytes as the events. A request batch that fits on events alone can still fail.
- On Soroban, ATTESTED 4-leg requests cost about 2.47M instructions per item, about 321K per verified leg. That alone would allow about 112 items with headroom, so instructions are no constraint next to events.
- Fees on testnet are dominated by rent, as measured on 2026-10-17 and not re-run. Extending each payout record for 31 days cost about 0.25 XLM per item: 2.46 of the 2.47 XLM charged for a 10-item request. The first call after deploy also extended the wasm and the instance, and paid 46 XLM. How long records are kept alive, and who pays for it, is a sponsorship cost question.

## Soroban measurements (test env, compiled wasm)

`share` is the binding resource as a share of its limit. "events" counts the events plus the return value, which is what the network charges. `footprint` is the number of distinct keys (disk reads plus in-memory reads). The SDK's own check adds the write entries on top of that, which is stricter than the network rule. `arg bytes` is the argument XDR alone; the `tx_size` share uses the envelope estimate described above.

Run 2026-11-02, base commit `4aa2be1`, prototype wasm sha256 `3094275c27ce8fe7b2fab3ddad4b9bf6e6350a17537343b0dc0b3e8591eb1a84`.

| op | mode | legs | fee | dest | items | instructions | memory | disk reads | mem reads | writes | read bytes | write bytes | event bytes | return bytes | arg bytes | footprint | binding | share |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| request_payouts | allowlist | 1 | yes | Account | 1 | 950170 | 1301935 | 0 | 9 | 3 | 0 | 1728 | 564 | 128 | 468 | 9 | events | 4.2% |
| execute_batch | allowlist | 1 | yes | Account | 1 | 1217473 | 1339087 | 2 | 9 | 7 | 232 | 1900 | 936 | 4 | 52 | 11 | events | 5.7% |
| request_payouts | allowlist | 1 | yes | Account | 10 | 3401037 | 1585201 | 0 | 36 | 12 | 0 | 7452 | 5640 | 1172 | 4572 | 36 | events | 41.6% |
| execute_batch | allowlist | 1 | yes | Account | 10 | 6742066 | 2139034 | 11 | 36 | 34 | 1276 | 6112 | 9360 | 4 | 412 | 47 | events | 57.2% |
| request_payouts | allowlist | 1 | yes | Account | 20 | 7109070 | 2135921 | 0 | 66 | 22 | 0 | 13812 | 11280 | 2332 | 9132 | 66 | events | 83.1% |
| execute_batch | allowlist | 1 | yes | Account | 20 | 14796476 | 3611164 | 21 | 66 | 64 | 2436 | 10792 | 18720 | 4 | 812 | 87 | events | 114.3% |
| request_payouts | allowlist | 1 | yes | Account | 40 | 17549570 | 3982561 | 0 | 126 | 42 | 0 | 26532 | 22560 | 4652 | 18252 | 126 | events | 166.1% |
| execute_batch | allowlist | 1 | yes | Account | 40 | 37032263 | 8397424 | 41 | 126 | 124 | 4756 | 20152 | 37440 | 4 | 1612 | 167 | events | 228.5% |
| request_payouts | allowlist | 2 | yes | Account | 1 | 1024317 | 1307184 | 0 | 10 | 3 | 0 | 1840 | 676 | 128 | 580 | 10 | events | 4.9% |
| execute_batch | allowlist | 2 | yes | Account | 1 | 1396969 | 1365388 | 3 | 10 | 8 | 348 | 2016 | 1288 | 4 | 52 | 13 | events | 7.9% |
| request_payouts | allowlist | 2 | yes | Account | 10 | 4489861 | 1720131 | 0 | 46 | 12 | 0 | 8572 | 6760 | 1172 | 5692 | 46 | events | 48.4% |
| execute_batch | allowlist | 2 | yes | Account | 10 | 9584187 | 2731354 | 21 | 46 | 44 | 2436 | 7272 | 12880 | 4 | 412 | 67 | events | 78.6% |
| request_payouts | allowlist | 2 | yes | Account | 20 | 10089879 | 2588981 | 0 | 86 | 22 | 0 | 16052 | 13520 | 2332 | 11372 | 86 | events | 96.8% |
| execute_batch | allowlist | 2 | yes | Account | 20 | 22777615 | 5528554 | 41 | 86 | 84 | 4756 | 13112 | 25760 | 4 | 812 | 127 | events | 157.3% |
| request_payouts | allowlist | 2 | yes | Account | 40 | 26457687 | 5621481 | 0 | 166 | 42 | 0 | 31012 | 27040 | 4652 | 22732 | 166 | events | 193.4% |
| execute_batch | allowlist | 2 | yes | Account | 40 | 61266234 | 15163354 | 81 | 166 | 164 | 9396 | 24792 | 51520 | 4 | 1612 | 247 | events | 314.5% |
| request_payouts | allowlist | 4 | yes | Account | 1 | 1218205 | 1318237 | 0 | 12 | 3 | 0 | 2064 | 900 | 128 | 804 | 12 | events | 6.3% |
| execute_batch | allowlist | 4 | yes | Account | 1 | 1792356 | 1422187 | 5 | 12 | 10 | 580 | 2248 | 1992 | 4 | 52 | 17 | events | 12.2% |
| request_payouts | allowlist | 4 | yes | Account | 10 | 7286102 | 2002291 | 0 | 66 | 12 | 0 | 10812 | 9000 | 1172 | 7932 | 66 | events | 62.1% |
| execute_batch | allowlist | 4 | yes | Account | 10 | 16564303 | 4328944 | 41 | 66 | 64 | 4756 | 9592 | 19920 | 4 | 412 | 107 | events | 121.6% |
| request_payouts | allowlist | 4 | yes | Account | 20 | 17344654 | 3534701 | 0 | 126 | 22 | 0 | 20532 | 18000 | 2332 | 15852 | 126 | events | 124.1% |
| execute_batch | allowlist | 4 | yes | Account | 20 | 43117697 | 11013634 | 81 | 126 | 124 | 9396 | 17752 | 39840 | 4 | 812 | 207 | events | 243.2% |
| request_payouts | allowlist | 4 | yes | Account | 40 | 47609047 | 9038521 | 0 | 246 | 42 | 0 | 39972 | 36000 | 4652 | 31692 | 246 | events | 248.1% |
| execute_batch | allowlist | 4 | yes | Account | 40 | 127228265 | 35293414 | 161 | 246 | 244 | 18676 | 34072 | 79680 | 4 | 1612 | 407 | events | 486.4% |
| request_payouts | attested | 1 | yes | Account | 1 | 1410912 | 1303238 | 0 | 8 | 3 | 0 | 1728 | 564 | 128 | 636 | 8 | events | 4.2% |
| execute_batch | attested | 1 | yes | Account | 1 | 1176543 | 1334352 | 2 | 8 | 7 | 232 | 1900 | 936 | 4 | 52 | 10 | events | 5.7% |
| request_payouts | attested | 1 | yes | Account | 10 | 7711478 | 1576793 | 0 | 26 | 12 | 0 | 7452 | 5640 | 1172 | 6252 | 26 | events | 41.6% |
| execute_batch | attested | 1 | yes | Account | 10 | 5834886 | 2038044 | 11 | 26 | 34 | 1276 | 6112 | 9360 | 4 | 412 | 37 | events | 57.2% |
| request_payouts | attested | 1 | yes | Account | 20 | 15192004 | 2050793 | 0 | 46 | 22 | 0 | 13812 | 11280 | 2332 | 12492 | 46 | events | 83.1% |
| execute_batch | attested | 1 | yes | Account | 20 | 12286089 | 3276114 | 21 | 46 | 64 | 2436 | 10792 | 18720 | 4 | 812 | 67 | events | 114.3% |
| request_payouts | attested | 1 | yes | Account | 40 | 31610789 | 3535793 | 0 | 86 | 42 | 0 | 26532 | 22560 | 4652 | 24972 | 86 | events | 166.1% |
| execute_batch | attested | 1 | yes | Account | 40 | 28725245 | 7192854 | 41 | 86 | 124 | 4756 | 20152 | 37440 | 4 | 1612 | 127 | events | 228.5% |
| request_payouts | attested | 2 | yes | Account | 1 | 1939517 | 1311397 | 0 | 8 | 3 | 0 | 1840 | 676 | 128 | 916 | 8 | events | 4.9% |
| execute_batch | attested | 2 | yes | Account | 1 | 1318427 | 1356745 | 3 | 8 | 8 | 348 | 2016 | 1288 | 4 | 52 | 11 | events | 7.9% |
| request_payouts | attested | 2 | yes | Account | 10 | 13059695 | 1707343 | 0 | 26 | 12 | 0 | 8572 | 6760 | 1172 | 9052 | 26 | events | 48.4% |
| execute_batch | attested | 2 | yes | Account | 10 | 7638500 | 2485804 | 21 | 26 | 44 | 2436 | 7272 | 12880 | 4 | 412 | 47 | events | 78.6% |
| request_payouts | attested | 2 | yes | Account | 20 | 26010630 | 2420693 | 0 | 46 | 22 | 0 | 16052 | 13520 | 2332 | 18092 | 46 | events | 96.8% |
| execute_batch | attested | 2 | yes | Account | 20 | 16629014 | 4669034 | 41 | 46 | 84 | 4756 | 13112 | 25760 | 4 | 812 | 87 | events | 157.3% |
| request_payouts | attested | 2 | yes | Account | 40 | 53682449 | 4710793 | 0 | 86 | 42 | 0 | 31012 | 27040 | 4652 | 36172 | 86 | events | 193.4% |
| execute_batch | attested | 2 | yes | Account | 40 | 40216175 | 11968294 | 81 | 86 | 164 | 9396 | 24792 | 51520 | 4 | 1612 | 167 | events | 314.5% |
| request_payouts | attested | 4 | yes | Account | 1 | 3040224 | 1328195 | 0 | 8 | 3 | 0 | 2064 | 900 | 128 | 1476 | 8 | events | 6.3% |
| execute_batch | attested | 4 | yes | Account | 1 | 1598906 | 1404129 | 5 | 8 | 10 | 580 | 2248 | 1992 | 4 | 52 | 13 | events | 12.2% |
| request_payouts | attested | 4 | yes | Account | 10 | 24168807 | 1973243 | 0 | 26 | 12 | 0 | 10812 | 9000 | 1172 | 14652 | 26 | events | 62.1% |
| execute_batch | attested | 4 | yes | Account | 10 | 11326971 | 3641124 | 41 | 26 | 64 | 4756 | 9592 | 19920 | 4 | 412 | 67 | events | 121.6% |
| request_payouts | attested | 4 | yes | Account | 20 | 48454806 | 3170093 | 0 | 46 | 22 | 0 | 20532 | 18000 | 2332 | 29292 | 46 | events | 124.1% |
| execute_batch | attested | 4 | yes | Account | 20 | 25801141 | 8494074 | 81 | 46 | 124 | 9396 | 17752 | 39840 | 4 | 812 | 127 | events | 243.2% |
| request_payouts | attested | 4 | yes | Account | 40 | 99546318 | 7079993 | 0 | 86 | 42 | 0 | 39972 | 36000 | 4652 | 58572 | 86 | events | 248.1% |
| execute_batch | attested | 4 | yes | Account | 40 | 66156990 | 25675974 | 161 | 86 | 244 | 18676 | 34072 | 79680 | 4 | 1612 | 247 | events | 486.4% |
| request_payouts | both | 1 | yes | Account | 1 | 1440943 | 1307539 | 0 | 9 | 3 | 0 | 1728 | 564 | 128 | 636 | 9 | events | 4.2% |
| execute_batch | both | 1 | yes | Account | 1 | 1217473 | 1339087 | 2 | 9 | 7 | 232 | 1900 | 936 | 4 | 52 | 11 | events | 5.7% |
| request_payouts | both | 1 | yes | Account | 10 | 8308767 | 1641241 | 0 | 36 | 12 | 0 | 7452 | 5640 | 1172 | 6252 | 36 | events | 41.6% |
| execute_batch | both | 1 | yes | Account | 10 | 6742066 | 2139034 | 11 | 36 | 34 | 1276 | 6112 | 9360 | 4 | 412 | 47 | events | 57.2% |
| request_payouts | both | 1 | yes | Account | 20 | 16924530 | 2248001 | 0 | 66 | 22 | 0 | 13812 | 11280 | 2332 | 12492 | 66 | events | 83.1% |
| execute_batch | both | 1 | yes | Account | 20 | 14796476 | 3611164 | 21 | 66 | 64 | 2436 | 10792 | 18720 | 4 | 812 | 87 | events | 114.3% |
| request_payouts | both | 1 | yes | Account | 40 | 37180490 | 4206721 | 0 | 126 | 42 | 0 | 26532 | 22560 | 4652 | 24972 | 126 | events | 166.1% |
| execute_batch | both | 1 | yes | Account | 40 | 37032263 | 8397424 | 41 | 126 | 124 | 4756 | 20152 | 37440 | 4 | 1612 | 167 | events | 228.5% |
| request_payouts | both | 2 | yes | Account | 1 | 2003060 | 1318392 | 0 | 10 | 3 | 0 | 1840 | 676 | 128 | 916 | 10 | events | 4.9% |
| execute_batch | both | 2 | yes | Account | 1 | 1396969 | 1365388 | 3 | 10 | 8 | 348 | 2016 | 1288 | 4 | 52 | 13 | events | 7.9% |
| request_payouts | both | 2 | yes | Account | 10 | 14277291 | 1832211 | 0 | 46 | 12 | 0 | 8572 | 6760 | 1172 | 9052 | 46 | events | 48.4% |
| execute_batch | both | 2 | yes | Account | 10 | 9584187 | 2731354 | 21 | 46 | 44 | 2436 | 7272 | 12880 | 4 | 412 | 67 | events | 78.6% |
| request_payouts | both | 2 | yes | Account | 20 | 29664739 | 2813141 | 0 | 86 | 22 | 0 | 16052 | 13520 | 2332 | 18092 | 86 | events | 96.8% |
| execute_batch | both | 2 | yes | Account | 20 | 22777615 | 5528554 | 41 | 86 | 84 | 4756 | 13112 | 25760 | 4 | 812 | 127 | events | 157.3% |
| request_payouts | both | 2 | yes | Account | 40 | 65607407 | 6069801 | 0 | 166 | 42 | 0 | 31012 | 27040 | 4652 | 36172 | 166 | events | 193.4% |
| execute_batch | both | 2 | yes | Account | 40 | 61266234 | 15163354 | 81 | 166 | 164 | 9396 | 24792 | 51520 | 4 | 1612 | 247 | events | 314.5% |
| request_payouts | both | 4 | yes | Account | 1 | 3172888 | 1340653 | 0 | 12 | 3 | 0 | 2064 | 900 | 128 | 1476 | 12 | events | 6.3% |
| execute_batch | both | 4 | yes | Account | 1 | 1792356 | 1422187 | 5 | 12 | 10 | 580 | 2248 | 1992 | 4 | 52 | 17 | events | 12.2% |
| request_payouts | both | 4 | yes | Account | 10 | 26832932 | 2226451 | 0 | 66 | 12 | 0 | 10812 | 9000 | 1172 | 14652 | 66 | events | 62.1% |
| execute_batch | both | 4 | yes | Account | 10 | 16564303 | 4328944 | 41 | 66 | 64 | 4756 | 9592 | 19920 | 4 | 412 | 107 | events | 121.6% |
| request_payouts | both | 4 | yes | Account | 20 | 56438314 | 3983021 | 0 | 126 | 22 | 0 | 20532 | 18000 | 2332 | 29292 | 126 | events | 124.1% |
| execute_batch | both | 4 | yes | Account | 20 | 43117697 | 11013634 | 81 | 126 | 124 | 9396 | 17752 | 39840 | 4 | 812 | 207 | events | 243.2% |
| request_payouts | both | 4 | yes | Account | 40 | 125796367 | 9935161 | 0 | 246 | 42 | 0 | 39972 | 36000 | 4652 | 58572 | 246 | events | 248.1% |
| execute_batch | both | 4 | yes | Account | 40 | 127228265 | 35293414 | 161 | 246 | 244 | 18676 | 34072 | 79680 | 4 | 1612 | 407 | events | 486.4% |
| request_payouts | allowlist | 1 | no | Account | 1 | 950170 | 1301935 | 0 | 9 | 3 | 0 | 1728 | 564 | 128 | 468 | 9 | events | 4.2% |
| execute_batch | allowlist | 1 | no | Account | 1 | 1088692 | 1319222 | 1 | 9 | 6 | 116 | 1784 | 696 | 4 | 52 | 10 | events | 4.3% |
| request_payouts | allowlist | 1 | no | Account | 10 | 3401037 | 1585201 | 0 | 36 | 12 | 0 | 7452 | 5640 | 1172 | 4572 | 36 | events | 41.6% |
| execute_batch | allowlist | 1 | no | Account | 10 | 5227987 | 1852247 | 10 | 36 | 33 | 1160 | 5996 | 6960 | 4 | 412 | 46 | events | 42.5% |
| request_payouts | allowlist | 1 | no | Account | 20 | 7109070 | 2135921 | 0 | 66 | 22 | 0 | 13812 | 11280 | 2332 | 9132 | 66 | events | 83.1% |
| execute_batch | allowlist | 1 | no | Account | 20 | 11342939 | 2833997 | 20 | 66 | 63 | 2320 | 10676 | 13920 | 4 | 812 | 86 | events | 85.0% |
| request_payouts | allowlist | 1 | no | Account | 40 | 17549570 | 3982561 | 0 | 126 | 42 | 0 | 26532 | 22560 | 4652 | 18252 | 126 | events | 166.1% |
| execute_batch | allowlist | 1 | no | Account | 40 | 28266861 | 6027497 | 40 | 126 | 123 | 4640 | 20036 | 27840 | 4 | 1612 | 166 | events | 169.9% |
| request_payouts | attested | 4 | yes | Contract | 1 | 3028705 | 1320603 | 0 | 8 | 3 | 0 | 2048 | 884 | 128 | 1460 | 8 | events | 6.2% |
| execute_batch | attested | 4 | yes | Contract | 1 | 1786236 | 1418461 | 2 | 12 | 10 | 208 | 2680 | 1960 | 4 | 52 | 14 | events | 12.0% |
| request_payouts | attested | 4 | yes | Contract | 10 | 23858444 | 1701483 | 0 | 26 | 12 | 0 | 10652 | 8840 | 1172 | 14492 | 26 | events | 61.1% |
| execute_batch | attested | 4 | yes | Contract | 10 | 14899144 | 3354100 | 2 | 66 | 64 | 208 | 13912 | 19600 | 4 | 412 | 68 | events | 119.7% |
| request_payouts | attested | 4 | yes | Contract | 20 | 47358435 | 2191373 | 0 | 46 | 22 | 0 | 20212 | 17680 | 2332 | 28972 | 46 | events | 122.1% |
| execute_batch | attested | 4 | yes | Contract | 20 | 35706062 | 6967810 | 2 | 126 | 124 | 208 | 26392 | 39200 | 4 | 812 | 128 | events | 239.3% |
| request_payouts | attested | 4 | yes | Contract | 40 | 95414727 | 3381753 | 0 | 86 | 42 | 0 | 39332 | 35360 | 4652 | 57932 | 86 | events | 244.2% |
| execute_batch | attested | 4 | yes | Contract | 40 | 95001734 | 18815230 | 2 | 246 | 244 | 208 | 51352 | 78400 | 4 | 1612 | 248 | events | 478.5% |
| request_payouts | allowlist | 1 | yes | Tagged | 1 | 956865 | 1302247 | 0 | 9 | 3 | 0 | 1732 | 568 | 128 | 472 | 9 | events | 4.2% |
| execute_batch | allowlist | 1 | yes | Tagged | 1 | 1230783 | 1340090 | 2 | 9 | 7 | 232 | 1900 | 1000 | 4 | 52 | 11 | events | 6.1% |
| request_payouts | allowlist | 1 | yes | Tagged | 10 | 3467987 | 1588321 | 0 | 36 | 12 | 0 | 7492 | 5680 | 1172 | 4612 | 36 | events | 41.8% |
| execute_batch | allowlist | 1 | yes | Tagged | 10 | 6875166 | 2149064 | 11 | 36 | 34 | 1276 | 6112 | 10000 | 4 | 412 | 47 | events | 61.1% |
| request_payouts | allowlist | 1 | yes | Tagged | 20 | 7242970 | 2142161 | 0 | 66 | 22 | 0 | 13892 | 11360 | 2332 | 9212 | 66 | events | 83.6% |
| execute_batch | allowlist | 1 | yes | Tagged | 20 | 15062676 | 3631224 | 21 | 66 | 64 | 2436 | 10792 | 20000 | 4 | 812 | 87 | events | 122.1% |
| request_payouts | allowlist | 1 | yes | Tagged | 40 | 17817370 | 3995041 | 0 | 126 | 42 | 0 | 26692 | 22720 | 4652 | 18412 | 126 | events | 167.1% |
| execute_batch | allowlist | 1 | yes | Tagged | 40 | 37564663 | 8437544 | 41 | 126 | 124 | 4756 | 20152 | 40000 | 4 | 1612 | 167 | events | 244.2% |
| request_payouts | allowlist | 2 | yes | Tagged | 1 | 1041039 | 1308016 | 0 | 10 | 3 | 0 | 1848 | 684 | 128 | 588 | 10 | events | 5.0% |
| execute_batch | allowlist | 2 | yes | Tagged | 1 | 1423589 | 1367394 | 3 | 10 | 8 | 348 | 2016 | 1416 | 4 | 52 | 13 | events | 8.7% |
| request_payouts | allowlist | 2 | yes | Tagged | 10 | 4657081 | 1728451 | 0 | 46 | 12 | 0 | 8652 | 6840 | 1172 | 5772 | 46 | events | 48.9% |
| execute_batch | allowlist | 2 | yes | Tagged | 10 | 9850387 | 2751414 | 21 | 46 | 44 | 2436 | 7272 | 14160 | 4 | 412 | 67 | events | 86.5% |
| request_payouts | allowlist | 2 | yes | Tagged | 20 | 10424319 | 2605621 | 0 | 86 | 22 | 0 | 16212 | 13680 | 2332 | 11532 | 86 | events | 97.7% |
| execute_batch | allowlist | 2 | yes | Tagged | 20 | 23310015 | 5568674 | 41 | 86 | 84 | 4756 | 13112 | 28320 | 4 | 812 | 127 | events | 172.9% |
| request_payouts | allowlist | 2 | yes | Tagged | 40 | 27126567 | 5654761 | 0 | 166 | 42 | 0 | 31332 | 27360 | 4652 | 23052 | 166 | events | 195.4% |
| execute_batch | allowlist | 2 | yes | Tagged | 40 | 62331034 | 15243594 | 81 | 166 | 164 | 9396 | 24792 | 56640 | 4 | 1612 | 247 | events | 345.7% |
| request_payouts | allowlist | 4 | yes | Tagged | 1 | 1264977 | 1320733 | 0 | 12 | 3 | 0 | 2080 | 916 | 128 | 820 | 12 | events | 6.4% |
| execute_batch | allowlist | 4 | yes | Tagged | 1 | 1845596 | 1426199 | 5 | 12 | 10 | 580 | 2248 | 2248 | 4 | 52 | 17 | events | 13.7% |
| request_payouts | allowlist | 4 | yes | Tagged | 10 | 7753822 | 2027251 | 0 | 66 | 12 | 0 | 10972 | 9160 | 1172 | 8092 | 66 | events | 63.1% |
| execute_batch | allowlist | 4 | yes | Tagged | 10 | 17096703 | 4369064 | 41 | 66 | 64 | 4756 | 9592 | 22480 | 4 | 412 | 107 | events | 137.2% |
| request_payouts | allowlist | 4 | yes | Tagged | 20 | 18280094 | 3584621 | 0 | 126 | 22 | 0 | 20852 | 18320 | 2332 | 16172 | 126 | events | 126.0% |
| execute_batch | allowlist | 4 | yes | Tagged | 20 | 44182497 | 11093874 | 81 | 126 | 124 | 9396 | 17752 | 44960 | 4 | 812 | 207 | events | 274.4% |
| request_payouts | allowlist | 4 | yes | Tagged | 40 | 49479927 | 9138361 | 0 | 246 | 42 | 0 | 40612 | 36640 | 4652 | 32332 | 246 | events | 252.0% |
| execute_batch | allowlist | 4 | yes | Tagged | 40 | 129357865 | 35453894 | 161 | 246 | 244 | 18676 | 34072 | 89920 | 4 | 1612 | 407 | events | 548.9% |
| request_payouts | attested | 4 | yes | Tagged | 1 | 3078072 | 1330275 | 0 | 8 | 3 | 0 | 2080 | 916 | 128 | 1492 | 8 | events | 6.4% |
| execute_batch | attested | 4 | yes | Tagged | 1 | 1625366 | 1406893 | 5 | 8 | 10 | 580 | 2248 | 2248 | 4 | 52 | 13 | events | 13.7% |
| request_payouts | attested | 4 | yes | Tagged | 10 | 24547287 | 1994043 | 0 | 26 | 12 | 0 | 10972 | 9160 | 1172 | 14812 | 26 | events | 63.1% |
| execute_batch | attested | 4 | yes | Tagged | 10 | 11591571 | 3668764 | 41 | 26 | 64 | 4756 | 9592 | 22480 | 4 | 412 | 67 | events | 137.2% |
| request_payouts | attested | 4 | yes | Tagged | 20 | 49211766 | 3211693 | 0 | 46 | 22 | 0 | 20852 | 18320 | 2332 | 29612 | 46 | events | 126.0% |
| execute_batch | attested | 4 | yes | Tagged | 20 | 26330341 | 8549354 | 81 | 46 | 124 | 9396 | 17752 | 44960 | 4 | 812 | 127 | events | 274.4% |
| request_payouts | attested | 4 | yes | Tagged | 40 | 101060238 | 7163193 | 0 | 86 | 42 | 0 | 40612 | 36640 | 4652 | 59212 | 86 | events | 252.0% |
| execute_batch | attested | 4 | yes | Tagged | 40 | 67215390 | 25786534 | 161 | 86 | 244 | 18676 | 34072 | 89920 | 4 | 1612 | 247 | events | 548.9% |
| request_payouts | allowlist | 2 | no | Tagged | 1 | 1041039 | 1308016 | 0 | 10 | 3 | 0 | 1848 | 684 | 128 | 588 | 10 | events | 5.0% |
| execute_batch | allowlist | 2 | no | Tagged | 1 | 1293312 | 1346817 | 2 | 10 | 7 | 232 | 1900 | 1176 | 4 | 52 | 12 | events | 7.2% |
| request_payouts | allowlist | 2 | no | Tagged | 10 | 4657081 | 1728451 | 0 | 46 | 12 | 0 | 8652 | 6840 | 1172 | 5772 | 46 | events | 48.9% |
| execute_batch | allowlist | 2 | no | Tagged | 10 | 8215778 | 2402652 | 20 | 46 | 43 | 2320 | 7156 | 11760 | 4 | 412 | 66 | events | 71.8% |
| request_payouts | allowlist | 2 | no | Tagged | 20 | 10424319 | 2605621 | 0 | 86 | 22 | 0 | 16212 | 13680 | 2332 | 11532 | 86 | events | 97.7% |
| execute_batch | allowlist | 2 | no | Tagged | 20 | 19330245 | 4545182 | 40 | 86 | 83 | 4640 | 12996 | 23520 | 4 | 812 | 126 | events | 143.6% |
| request_payouts | allowlist | 2 | no | Tagged | 40 | 27126567 | 5654761 | 0 | 166 | 42 | 0 | 31332 | 27360 | 4652 | 23052 | 166 | events | 195.4% |
| execute_batch | allowlist | 2 | no | Tagged | 40 | 51632741 | 11891442 | 80 | 166 | 163 | 9280 | 24676 | 47040 | 4 | 1612 | 246 | events | 287.1% |

Largest batch that keeps every resource at or under 70 percent of its limit, from a straight line through the 1 and 40 item points (every resource here is linear in the item count):

Run 2026-11-02, base commit `4aa2be1`, prototype wasm sha256 `3094275c27ce8fe7b2fab3ddad4b9bf6e6350a17537343b0dc0b3e8591eb1a84`.

| mode | legs | fee | dest | request_payouts | execute_batch |
| --- | --- | --- | --- | --- | --- |
| allowlist | 1 | yes | Account | 16 (events) | 12 (events) |
| allowlist | 2 | yes | Account | 14 (events) | 8 (events) |
| allowlist | 4 | yes | Account | 11 (events) | 5 (events) |
| attested | 1 | yes | Account | 16 (events) | 12 (events) |
| attested | 2 | yes | Account | 14 (events) | 8 (events) |
| attested | 4 | yes | Account | 11 (events) | 5 (events) |
| both | 1 | yes | Account | 16 (events) | 12 (events) |
| both | 2 | yes | Account | 14 (events) | 8 (events) |
| both | 4 | yes | Account | 11 (events) | 5 (events) |
| allowlist | 1 | no | Account | 16 (events) | 16 (events) |
| attested | 4 | yes | Contract | 11 (events) | 5 (events) |
| allowlist | 1 | yes | Tagged | 16 (events) | 11 (events) |
| allowlist | 2 | yes | Tagged | 14 (events) | 8 (events) |
| allowlist | 4 | yes | Tagged | 11 (events) | 5 (events) |
| attested | 4 | yes | Tagged | 11 (events) | 5 (events) |
| allowlist | 2 | no | Tagged | 14 (events) | 9 (events) |

Transfer budget sweep (`transfer_cap_sweep`): 10 items, every transfer a leg, no fee, then one leg per item:

Run 2026-11-02, base commit `4aa2be1`, prototype wasm sha256 `3094275c27ce8fe7b2fab3ddad4b9bf6e6350a17537343b0dc0b3e8591eb1a84`.

| op | mode | legs | fee | dest | items | instructions | memory | disk reads | mem reads | writes | read bytes | write bytes | event bytes | return bytes | arg bytes | footprint | binding | share |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| execute_batch | allowlist | 16 in all | 0 of 10 | Account | 10 | 6819340 | 2154576 | 16 | 42 | 39 | 1856 | 6692 | 9072 | 4 | 412 | 58 | events | 55.4% |
| execute_batch | allowlist | 16 in all | 0 of 10 | Tagged | 10 | 7032300 | 2170624 | 16 | 42 | 39 | 1856 | 6692 | 10096 | 4 | 412 | 58 | events | 61.6% |
| execute_batch | allowlist | 17 in all | 0 of 10 | Account | 10 | 7103104 | 2209594 | 17 | 43 | 40 | 1972 | 6808 | 9424 | 4 | 412 | 60 | events | 57.5% |
| execute_batch | allowlist | 17 in all | 0 of 10 | Tagged | 10 | 7329374 | 2226645 | 17 | 43 | 40 | 1972 | 6808 | 10512 | 4 | 412 | 60 | events | 64.2% |
| execute_batch | allowlist | 18 in all | 0 of 10 | Account | 10 | 7380894 | 2265936 | 18 | 44 | 41 | 2088 | 6924 | 9776 | 4 | 412 | 62 | events | 59.7% |
| execute_batch | allowlist | 18 in all | 0 of 10 | Tagged | 10 | 7620474 | 2283990 | 18 | 44 | 41 | 2088 | 6924 | 10928 | 4 | 412 | 62 | events | 66.7% |
| execute_batch | allowlist | 19 in all | 0 of 10 | Account | 10 | 7665101 | 2323602 | 19 | 45 | 42 | 2204 | 7040 | 10128 | 4 | 412 | 64 | events | 61.8% |
| execute_batch | allowlist | 19 in all | 0 of 10 | Tagged | 10 | 7917991 | 2342659 | 19 | 45 | 42 | 2204 | 7040 | 11344 | 4 | 412 | 64 | events | 69.3% |
| execute_batch | allowlist | 2 | no | Account | 10 | 7949578 | 2382592 | 20 | 46 | 43 | 2320 | 7156 | 10480 | 4 | 412 | 66 | events | 64.0% |
| execute_batch | allowlist | 2 | no | Tagged | 10 | 8215778 | 2402652 | 20 | 46 | 43 | 2320 | 7156 | 11760 | 4 | 412 | 66 | events | 71.8% |
| execute_batch | allowlist | 1 | no | Account | 10 | 5227987 | 1852247 | 10 | 36 | 33 | 1160 | 5996 | 6960 | 4 | 412 | 46 | events | 42.5% |
| execute_batch | allowlist | 1 | no | Tagged | 10 | 5361087 | 1862277 | 10 | 36 | 33 | 1160 | 5996 | 7600 | 4 | 412 | 46 | events | 46.4% |

The worst cases under the recommended limits (`recommended_limits_keep_headroom`; each is asserted at or under 70 percent of every limit):

Run 2026-11-02, base commit `4aa2be1`, prototype wasm sha256 `3094275c27ce8fe7b2fab3ddad4b9bf6e6350a17537343b0dc0b3e8591eb1a84`.

| op | mode | legs | fee | dest | items | instructions | memory | disk reads | mem reads | writes | read bytes | write bytes | event bytes | return bytes | arg bytes | footprint | binding | share |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| request_payouts | allowlist | 18 in all | 0 of 10 | Tagged | 10 | 4426581 | 1700425 | 0 | 44 | 12 | 0 | 8420 | 6608 | 1172 | 5540 | 44 | events | 47.5% |
| execute_batch | allowlist | 18 in all | 0 of 10 | Tagged | 10 | 7620474 | 2283990 | 18 | 44 | 41 | 2088 | 6924 | 10928 | 4 | 412 | 62 | events | 66.7% |
| request_payouts | allowlist | 18 in all | 0 of 10 | Account | 10 | 4279415 | 1693145 | 0 | 44 | 12 | 0 | 8348 | 6536 | 1172 | 5468 | 44 | events | 47.0% |
| execute_batch | allowlist | 18 in all | 0 of 10 | Account | 10 | 7380894 | 2265936 | 18 | 44 | 41 | 2088 | 6924 | 9776 | 4 | 412 | 62 | events | 59.7% |
| request_payouts | both | 16 in all | 2 of 4 | Tagged | 4 | 10981254 | 1576061 | 0 | 30 | 6 | 0 | 5044 | 3664 | 476 | 5932 | 30 | events | 25.3% |
| execute_batch | both | 16 in all | 2 of 4 | Tagged | 4 | 5656343 | 2020468 | 17 | 30 | 28 | 1972 | 4696 | 8512 | 4 | 172 | 47 | events | 52.0% |
| request_payouts | both | 4 | yes | Tagged | 3 | 8361118 | 1490571 | 0 | 24 | 5 | 0 | 4056 | 2748 | 360 | 4452 | 24 | events | 19.0% |
| execute_batch | both | 4 | yes | Tagged | 3 | 4438972 | 1825593 | 13 | 24 | 22 | 1508 | 3880 | 6744 | 4 | 132 | 37 | events | 41.2% |
| request_payouts | allowlist | 1 | yes | Tagged | 9 | 3146406 | 1546599 | 0 | 33 | 11 | 0 | 6852 | 5112 | 1056 | 4152 | 33 | events | 37.6% |
| execute_batch | allowlist | 1 | yes | Tagged | 9 | 6114019 | 2034618 | 10 | 33 | 31 | 1160 | 5644 | 9000 | 4 | 372 | 43 | events | 55.0% |
| request_payouts | both | 4 | no | Tagged | 10 | 27479212 | 2259731 | 0 | 66 | 12 | 0 | 10972 | 9160 | 1172 | 14812 | 66 | events | 63.1% |
| request_payouts | attested | 4 | yes | Tagged | 10 | 24547287 | 1994043 | 0 | 26 | 12 | 0 | 10972 | 9160 | 1172 | 14812 | 26 | events | 63.1% |
| request_payouts | both | 4 | no | Account | 10 | 26832932 | 2226451 | 0 | 66 | 12 | 0 | 10812 | 9000 | 1172 | 14652 | 66 | events | 62.1% |

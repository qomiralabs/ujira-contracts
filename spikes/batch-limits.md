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

| Part of an `execute_batch` item | Event bytes |
| --- | --- |
| `payout_executed` base | 344 |
| a leg to a classic account (its SAC event plus its `legs` entry) | 352 |
| a leg to a tagged (muxed `M`) destination, which adds `to_muxed_id` and a wider `legs` entry | **416** |
| a fee transfer (the fee address is never tagged) | 240 |

Tagged legs are the worst case, so the cap is set on them. The worst plan for a transfer budget puts as many items as `MAX_BATCH` allows and makes every transfer a tagged leg (`transfer_cap_sweep`):

| Transfers, 10 items, all tagged legs, no fee | Event bytes (with return value) | Share | Testnet |
| --- | --- | --- | --- |
| 16 | 10,100 | 61.6% | |
| 17 | 10,516 | 64.2% | |
| **18** | **10,932** | **66.7%** | **11,004, 67.2% (tx `c5fa1fd2...`, ledger 5248153)** |
| 19 | 11,348 | 69.3% | |
| 20 (the earlier proposal) | 11,764 | 71.8% | 11,844, 72.3% (tx `3c52d0d9...`, ledger 5248162) |

19 is 69.3 percent in the test env, and the network runs about half a point above the env, so it leaves no margin at all. 18 keeps 32.8 percent on testnet. With `MAX_BATCH = 20` alone, a batch of 4-leg payouts with a fee needs 39,844 event bytes, 243 percent of the limit. A single `MAX_BATCH` of 5 would fit the worst case, but it would halve throughput for the common 1-leg payout for no reason.

Headroom under the recommended limits (at least 30 percent required on every resource):

| Worst case under the limits | Measured on | Binding resource | Use | Headroom |
| --- | --- | --- | --- | --- |
| `execute_batch`, 10 items, 18 tagged legs, no fee | test env | events | 10,932 of 16,384 bytes, 66.7% | 33.3% |
| same, submitted to testnet (tx `c5fa1fd2...`, ledger 5248153) | testnet | events | 11,004 bytes, 67.2% | 32.8% |
| `execute_batch`, 10 items, 18 untagged legs, no fee | test env | events | 9,780 bytes, 59.7% | 40.3% |
| `execute_batch`, 4 items x 4 tagged legs, 2 with a fee (18 transfers) | test env | events | 8,516 bytes, 52.0% | 48.0% |
| `execute_batch`, 9 items x (1 tagged leg + fee) | test env | events | 9,004 bytes, 55.0% | 45.0% |
| `request_payouts`, 10 items x 4 tagged legs, BOTH mode | test env | events | 10,332 bytes, 63.1% (instructions 103.8M, 26.0%) | 36.9% |

The test suite asserts that every case in the table stays at or under 70 percent of every limit (`recommended_limits_keep_headroom`). Every case is measured, including plans that use exactly `MAX_BATCH_TRANSFERS` transfers, so a protocol or SDK change that eats the margin fails CI.

## Limits compared against

Soroban, per transaction, read with `stellar network settings` on 2026-10-17. Testnet (ledger 5243815) and mainnet (ledger 64991374, through `https://mainnet.sorobanrpc.com`) agree on every per-transaction value, both on protocol 29:

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

- `soroban/spikes/batch-limits/proto` holds the contract. Its compiled wasm is committed as `soroban/spikes/batch-limits/fixtures/batch_limits_proto.wasm` (sha256 `03132e3672d80f41...`; rebuild with `stellar contract build --package batch-limits-proto`). `soroban/spikes/batch-limits` loads that wasm into the SDK test env, because the native build meters host functions only and misses most of the cost. The token is a real SAC. Destinations are fresh classic accounts with trustlines, one per leg, so every transfer touches a new trustline. Tagged series pay the same kind of account through a muxed `M` address with a full 64-bit id. One series uses contract addresses instead. Attestations are real secp256k1 signatures, one per leg, over the attestation digest that binds chain, mandate, recipient, country policy, expiry, destination and tag. Resources come from the env's cost estimate, plus the XDR size of the return value, because the network counts that against the events limit. The `tx_size` share is an estimate of the whole envelope: the arguments twice (the auth entry repeats the call), plus 100 bytes per footprint key and 400 for the rest. That is calibrated on testnet, where a 16-item 4-leg request had a 47,016 byte envelope and the estimate gives 46,976. It is never the binding resource.
- Batch sizes 1, 10, 20 and 40; 1, 2 and 4 legs; ALLOWLIST, ATTESTED and BOTH modes; a fee on every row except one 1-leg series. Every item is checked to land APPROVED and then EXECUTED.
- The suite was run twice; the numbers are identical.

```
cd soroban && cargo test -p batch-limits-spike --release -- --nocapture
```

## Network cross-checks

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

- The return value of `request_payouts` (an `ItemResult` per item, about 116 bytes each) is charged against the same 16,384 bytes as the events. A request batch that fits on events alone can still fail.
- On Soroban, ATTESTED 4-leg requests cost about 10.05M instructions per item, almost all signature recovery. That alone would allow about 27 items with headroom, so instructions are the second constraint after events.
- Fees on testnet are dominated by rent. Extending each payout record for 31 days cost about 0.25 XLM per item: 2.46 of the 2.47 XLM charged for a 10-item request. The first call after deploy also extended the wasm and the instance, and paid 46 XLM. How long records are kept alive, and who pays for it, is a sponsorship cost question.

## Soroban measurements (test env, compiled wasm)

`share` is the binding resource as a share of its limit. "events" counts the events plus the return value, which is what the network charges. `footprint` is the number of distinct keys (disk reads plus in-memory reads). The SDK's own check adds the write entries on top of that, which is stricter than the network rule. `arg bytes` is the argument XDR alone; the `tx_size` share uses the envelope estimate described above.

| op | mode | legs | fee | dest | items | instructions | memory | disk reads | mem reads | writes | read bytes | write bytes | event bytes | return bytes | arg bytes | footprint | binding | share |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| request_payouts | allowlist | 1 | yes | Account | 1 | 962688 | 1304179 | 0 | 9 | 3 | 0 | 1756 | 564 | 128 | 468 | 9 | events | 4.2% |
| execute_batch | allowlist | 1 | yes | Account | 1 | 1229783 | 1341331 | 2 | 9 | 7 | 232 | 1932 | 936 | 4 | 52 | 11 | events | 5.7% |
| request_payouts | allowlist | 1 | yes | Account | 10 | 3409919 | 1587445 | 0 | 36 | 12 | 0 | 7444 | 5640 | 1172 | 4572 | 36 | events | 41.6% |
| execute_batch | allowlist | 1 | yes | Account | 10 | 6748652 | 2141278 | 11 | 36 | 34 | 1276 | 6144 | 9360 | 4 | 412 | 47 | events | 57.2% |
| request_payouts | allowlist | 1 | yes | Account | 20 | 7113912 | 2138165 | 0 | 66 | 22 | 0 | 13764 | 11280 | 2332 | 9132 | 66 | events | 83.1% |
| execute_batch | allowlist | 1 | yes | Account | 20 | 14796702 | 3613408 | 21 | 66 | 64 | 2436 | 10824 | 18720 | 4 | 812 | 87 | events | 114.3% |
| request_payouts | allowlist | 1 | yes | Account | 40 | 17546332 | 3984805 | 0 | 126 | 42 | 0 | 26404 | 22560 | 4652 | 18252 | 126 | events | 166.1% |
| execute_batch | allowlist | 1 | yes | Account | 40 | 37019769 | 8399668 | 41 | 126 | 124 | 4756 | 20184 | 37440 | 4 | 1612 | 167 | events | 228.5% |
| request_payouts | allowlist | 2 | yes | Account | 1 | 1036835 | 1309428 | 0 | 10 | 3 | 0 | 1868 | 676 | 128 | 580 | 10 | events | 4.9% |
| execute_batch | allowlist | 2 | yes | Account | 1 | 1409279 | 1367632 | 3 | 10 | 8 | 348 | 2048 | 1288 | 4 | 52 | 13 | events | 7.9% |
| request_payouts | allowlist | 2 | yes | Account | 10 | 4498743 | 1722375 | 0 | 46 | 12 | 0 | 8564 | 6760 | 1172 | 5692 | 46 | events | 48.4% |
| execute_batch | allowlist | 2 | yes | Account | 10 | 9590773 | 2733598 | 21 | 46 | 44 | 2436 | 7304 | 12880 | 4 | 412 | 67 | events | 78.6% |
| request_payouts | allowlist | 2 | yes | Account | 20 | 10094721 | 2591225 | 0 | 86 | 22 | 0 | 16004 | 13520 | 2332 | 11372 | 86 | events | 96.8% |
| execute_batch | allowlist | 2 | yes | Account | 20 | 22777841 | 5530798 | 41 | 86 | 84 | 4756 | 13144 | 25760 | 4 | 812 | 127 | events | 157.3% |
| request_payouts | allowlist | 2 | yes | Account | 40 | 26454449 | 5623725 | 0 | 166 | 42 | 0 | 30884 | 27040 | 4652 | 22732 | 166 | events | 193.4% |
| execute_batch | allowlist | 2 | yes | Account | 40 | 61253740 | 15165598 | 81 | 166 | 164 | 9396 | 24824 | 51520 | 4 | 1612 | 247 | events | 314.5% |
| request_payouts | allowlist | 4 | yes | Account | 1 | 1230723 | 1320481 | 0 | 12 | 3 | 0 | 2092 | 900 | 128 | 804 | 12 | events | 6.3% |
| execute_batch | allowlist | 4 | yes | Account | 1 | 1804666 | 1424431 | 5 | 12 | 10 | 580 | 2280 | 1992 | 4 | 52 | 17 | events | 12.2% |
| request_payouts | allowlist | 4 | yes | Account | 10 | 7294984 | 2004535 | 0 | 66 | 12 | 0 | 10804 | 9000 | 1172 | 7932 | 66 | events | 62.1% |
| execute_batch | allowlist | 4 | yes | Account | 10 | 16570889 | 4331188 | 41 | 66 | 64 | 4756 | 9624 | 19920 | 4 | 412 | 107 | events | 121.6% |
| request_payouts | allowlist | 4 | yes | Account | 20 | 17349496 | 3536945 | 0 | 126 | 22 | 0 | 20484 | 18000 | 2332 | 15852 | 126 | events | 124.1% |
| execute_batch | allowlist | 4 | yes | Account | 20 | 43117923 | 11015878 | 81 | 126 | 124 | 9396 | 17784 | 39840 | 4 | 812 | 207 | events | 243.2% |
| request_payouts | allowlist | 4 | yes | Account | 40 | 47605809 | 9040765 | 0 | 246 | 42 | 0 | 39844 | 36000 | 4652 | 31692 | 246 | events | 248.1% |
| execute_batch | allowlist | 4 | yes | Account | 40 | 127215771 | 35295658 | 161 | 246 | 244 | 18676 | 34104 | 79680 | 4 | 1612 | 407 | events | 486.4% |
| request_payouts | attested | 1 | yes | Account | 1 | 3334433 | 1306082 | 0 | 8 | 3 | 0 | 1756 | 564 | 128 | 608 | 8 | events | 4.2% |
| execute_batch | attested | 1 | yes | Account | 1 | 1188853 | 1336596 | 2 | 8 | 7 | 232 | 1932 | 936 | 4 | 52 | 10 | events | 5.7% |
| request_payouts | attested | 1 | yes | Account | 10 | 26830390 | 1585037 | 0 | 26 | 12 | 0 | 7444 | 5640 | 1172 | 5972 | 26 | events | 41.6% |
| execute_batch | attested | 1 | yes | Account | 10 | 5841472 | 2040288 | 11 | 26 | 34 | 1276 | 6144 | 9360 | 4 | 412 | 37 | events | 57.2% |
| request_payouts | attested | 1 | yes | Account | 20 | 53417010 | 2065037 | 0 | 46 | 22 | 0 | 13764 | 11280 | 2332 | 11932 | 46 | events | 83.1% |
| execute_batch | attested | 1 | yes | Account | 20 | 12286315 | 3278358 | 21 | 46 | 64 | 2436 | 10824 | 18720 | 4 | 812 | 67 | events | 114.3% |
| request_payouts | attested | 1 | yes | Account | 40 | 108047775 | 3562037 | 0 | 86 | 42 | 0 | 26404 | 22560 | 4652 | 23852 | 86 | events | 166.1% |
| execute_batch | attested | 1 | yes | Account | 40 | 28712751 | 7195098 | 41 | 86 | 124 | 4756 | 20184 | 37440 | 4 | 1612 | 127 | events | 228.5% |
| request_payouts | attested | 2 | yes | Account | 1 | 5770401 | 1314841 | 0 | 8 | 3 | 0 | 1868 | 676 | 128 | 860 | 8 | events | 4.9% |
| execute_batch | attested | 2 | yes | Account | 1 | 1330737 | 1358989 | 3 | 8 | 8 | 348 | 2048 | 1288 | 4 | 52 | 11 | events | 7.9% |
| request_payouts | attested | 2 | yes | Account | 10 | 51252237 | 1721587 | 0 | 26 | 12 | 0 | 8564 | 6760 | 1172 | 8492 | 26 | events | 48.4% |
| execute_batch | attested | 2 | yes | Account | 10 | 7645086 | 2488048 | 21 | 26 | 44 | 2436 | 7304 | 12880 | 4 | 412 | 47 | events | 78.6% |
| request_payouts | attested | 2 | yes | Account | 20 | 102383000 | 2446937 | 0 | 46 | 22 | 0 | 16004 | 13520 | 2332 | 16972 | 46 | events | 96.8% |
| execute_batch | attested | 2 | yes | Account | 20 | 16629240 | 4671278 | 41 | 46 | 84 | 4756 | 13144 | 25760 | 4 | 812 | 87 | events | 157.3% |
| request_payouts | attested | 2 | yes | Account | 40 | 206414163 | 4761037 | 0 | 86 | 42 | 0 | 30884 | 27040 | 4652 | 33932 | 86 | events | 193.4% |
| execute_batch | attested | 2 | yes | Account | 40 | 40203681 | 11970538 | 81 | 86 | 164 | 9396 | 24824 | 51520 | 4 | 1612 | 167 | events | 314.5% |
| request_payouts | attested | 4 | yes | Account | 1 | 10685834 | 1332839 | 0 | 8 | 3 | 0 | 2092 | 900 | 128 | 1364 | 8 | events | 6.3% |
| execute_batch | attested | 4 | yes | Account | 1 | 1611216 | 1406373 | 5 | 8 | 10 | 580 | 2280 | 1992 | 4 | 52 | 13 | events | 12.2% |
| request_payouts | attested | 4 | yes | Account | 10 | 100508609 | 1999487 | 0 | 26 | 12 | 0 | 10804 | 9000 | 1172 | 13532 | 26 | events | 62.1% |
| execute_batch | attested | 4 | yes | Account | 10 | 11333557 | 3643368 | 41 | 26 | 64 | 4756 | 9624 | 19920 | 4 | 412 | 67 | events | 121.6% |
| request_payouts | attested | 4 | yes | Account | 20 | 201121696 | 3220337 | 0 | 46 | 22 | 0 | 20484 | 18000 | 2332 | 27052 | 46 | events | 124.1% |
| execute_batch | attested | 4 | yes | Account | 20 | 25801367 | 8496318 | 81 | 46 | 124 | 9396 | 17784 | 39840 | 4 | 812 | 127 | events | 243.2% |
| request_payouts | attested | 4 | yes | Account | 40 | 404867072 | 7178237 | 0 | 86 | 42 | 0 | 39844 | 36000 | 4652 | 54092 | 86 | events | 248.1% |
| execute_batch | attested | 4 | yes | Account | 40 | 66144496 | 25678218 | 161 | 86 | 244 | 18676 | 34104 | 79680 | 4 | 1612 | 247 | events | 486.4% |
| request_payouts | both | 1 | yes | Account | 1 | 3364464 | 1310383 | 0 | 9 | 3 | 0 | 1756 | 564 | 128 | 608 | 9 | events | 4.2% |
| execute_batch | both | 1 | yes | Account | 1 | 1229783 | 1341331 | 2 | 9 | 7 | 232 | 1932 | 936 | 4 | 52 | 11 | events | 5.7% |
| request_payouts | both | 1 | yes | Account | 10 | 27427679 | 1649485 | 0 | 36 | 12 | 0 | 7444 | 5640 | 1172 | 5972 | 36 | events | 41.6% |
| execute_batch | both | 1 | yes | Account | 10 | 6748652 | 2141278 | 11 | 36 | 34 | 1276 | 6144 | 9360 | 4 | 412 | 47 | events | 57.2% |
| request_payouts | both | 1 | yes | Account | 20 | 55149536 | 2262245 | 0 | 66 | 22 | 0 | 13764 | 11280 | 2332 | 11932 | 66 | events | 83.1% |
| execute_batch | both | 1 | yes | Account | 20 | 14796702 | 3613408 | 21 | 66 | 64 | 2436 | 10824 | 18720 | 4 | 812 | 87 | events | 114.3% |
| request_payouts | both | 1 | yes | Account | 40 | 113617476 | 4232965 | 0 | 126 | 42 | 0 | 26404 | 22560 | 4652 | 23852 | 126 | events | 166.1% |
| execute_batch | both | 1 | yes | Account | 40 | 37019769 | 8399668 | 41 | 126 | 124 | 4756 | 20184 | 37440 | 4 | 1612 | 167 | events | 228.5% |
| request_payouts | both | 2 | yes | Account | 1 | 5833944 | 1321836 | 0 | 10 | 3 | 0 | 1868 | 676 | 128 | 860 | 10 | events | 4.9% |
| execute_batch | both | 2 | yes | Account | 1 | 1409279 | 1367632 | 3 | 10 | 8 | 348 | 2048 | 1288 | 4 | 52 | 13 | events | 7.9% |
| request_payouts | both | 2 | yes | Account | 10 | 52469833 | 1846455 | 0 | 46 | 12 | 0 | 8564 | 6760 | 1172 | 8492 | 46 | events | 48.4% |
| execute_batch | both | 2 | yes | Account | 10 | 9590773 | 2733598 | 21 | 46 | 44 | 2436 | 7304 | 12880 | 4 | 412 | 67 | events | 78.6% |
| request_payouts | both | 2 | yes | Account | 20 | 106037109 | 2839385 | 0 | 86 | 22 | 0 | 16004 | 13520 | 2332 | 16972 | 86 | events | 96.8% |
| execute_batch | both | 2 | yes | Account | 20 | 22777841 | 5530798 | 41 | 86 | 84 | 4756 | 13144 | 25760 | 4 | 812 | 127 | events | 157.3% |
| request_payouts | both | 2 | yes | Account | 40 | 218339121 | 6120045 | 0 | 166 | 42 | 0 | 30884 | 27040 | 4652 | 33932 | 166 | events | 193.4% |
| execute_batch | both | 2 | yes | Account | 40 | 61253740 | 15165598 | 81 | 166 | 164 | 9396 | 24824 | 51520 | 4 | 1612 | 247 | events | 314.5% |
| request_payouts | both | 4 | yes | Account | 1 | 10818498 | 1345297 | 0 | 12 | 3 | 0 | 2092 | 900 | 128 | 1364 | 12 | events | 6.3% |
| execute_batch | both | 4 | yes | Account | 1 | 1804666 | 1424431 | 5 | 12 | 10 | 580 | 2280 | 1992 | 4 | 52 | 17 | events | 12.2% |
| request_payouts | both | 4 | yes | Account | 10 | 103172734 | 2252695 | 0 | 66 | 12 | 0 | 10804 | 9000 | 1172 | 13532 | 66 | events | 62.1% |
| execute_batch | both | 4 | yes | Account | 10 | 16570889 | 4331188 | 41 | 66 | 64 | 4756 | 9624 | 19920 | 4 | 412 | 107 | events | 121.6% |
| request_payouts | both | 4 | yes | Account | 20 | 209105204 | 4033265 | 0 | 126 | 22 | 0 | 20484 | 18000 | 2332 | 27052 | 126 | events | 124.1% |
| execute_batch | both | 4 | yes | Account | 20 | 43117923 | 11015878 | 81 | 126 | 124 | 9396 | 17784 | 39840 | 4 | 812 | 207 | events | 243.2% |
| request_payouts | both | 4 | yes | Account | 40 | 431117121 | 10033405 | 0 | 246 | 42 | 0 | 39844 | 36000 | 4652 | 54092 | 246 | events | 248.1% |
| execute_batch | both | 4 | yes | Account | 40 | 127215771 | 35295658 | 161 | 246 | 244 | 18676 | 34104 | 79680 | 4 | 1612 | 407 | events | 486.4% |
| request_payouts | allowlist | 1 | no | Account | 1 | 962688 | 1304179 | 0 | 9 | 3 | 0 | 1756 | 564 | 128 | 468 | 9 | events | 4.2% |
| execute_batch | allowlist | 1 | no | Account | 1 | 1101002 | 1321466 | 1 | 9 | 6 | 116 | 1816 | 696 | 4 | 52 | 10 | events | 4.3% |
| request_payouts | allowlist | 1 | no | Account | 10 | 3409919 | 1587445 | 0 | 36 | 12 | 0 | 7444 | 5640 | 1172 | 4572 | 36 | events | 41.6% |
| execute_batch | allowlist | 1 | no | Account | 10 | 5234573 | 1854491 | 10 | 36 | 33 | 1160 | 6028 | 6960 | 4 | 412 | 46 | events | 42.5% |
| request_payouts | allowlist | 1 | no | Account | 20 | 7113912 | 2138165 | 0 | 66 | 22 | 0 | 13764 | 11280 | 2332 | 9132 | 66 | events | 83.1% |
| execute_batch | allowlist | 1 | no | Account | 20 | 11343165 | 2836241 | 20 | 66 | 63 | 2320 | 10708 | 13920 | 4 | 812 | 86 | events | 85.0% |
| request_payouts | allowlist | 1 | no | Account | 40 | 17546332 | 3984805 | 0 | 126 | 42 | 0 | 26404 | 22560 | 4652 | 18252 | 126 | events | 166.1% |
| execute_batch | allowlist | 1 | no | Account | 40 | 28254367 | 6029741 | 40 | 126 | 123 | 4640 | 20068 | 27840 | 4 | 1612 | 166 | events | 169.9% |
| request_payouts | attested | 4 | yes | Contract | 1 | 10674451 | 1325247 | 0 | 8 | 3 | 0 | 2076 | 884 | 128 | 1348 | 8 | events | 6.2% |
| execute_batch | attested | 4 | yes | Contract | 1 | 1798546 | 1420705 | 2 | 12 | 10 | 208 | 2712 | 1960 | 4 | 52 | 14 | events | 12.0% |
| request_payouts | attested | 4 | yes | Contract | 10 | 100199814 | 1727727 | 0 | 26 | 12 | 0 | 10644 | 8840 | 1172 | 13372 | 26 | events | 61.1% |
| execute_batch | attested | 4 | yes | Contract | 10 | 14905730 | 3356344 | 2 | 66 | 64 | 208 | 13944 | 19600 | 4 | 412 | 68 | events | 119.7% |
| request_payouts | attested | 4 | yes | Contract | 20 | 200028045 | 2241617 | 0 | 46 | 22 | 0 | 20164 | 17680 | 2332 | 26732 | 46 | events | 122.1% |
| execute_batch | attested | 4 | yes | Contract | 20 | 35706288 | 6970054 | 2 | 126 | 124 | 208 | 26424 | 39200 | 4 | 812 | 128 | events | 239.3% |
| request_payouts | attested | 4 | yes | Contract | 40 | 400741129 | 3479997 | 0 | 86 | 42 | 0 | 39204 | 35360 | 4652 | 53452 | 86 | events | 244.2% |
| execute_batch | attested | 4 | yes | Contract | 40 | 94989240 | 18817474 | 2 | 246 | 244 | 208 | 51384 | 78400 | 4 | 1612 | 248 | events | 478.5% |
| request_payouts | allowlist | 1 | yes | Tagged | 1 | 969383 | 1304491 | 0 | 9 | 3 | 0 | 1760 | 568 | 128 | 472 | 9 | events | 4.2% |
| execute_batch | allowlist | 1 | yes | Tagged | 1 | 1243093 | 1342334 | 2 | 9 | 7 | 232 | 1932 | 1000 | 4 | 52 | 11 | events | 6.1% |
| request_payouts | allowlist | 1 | yes | Tagged | 10 | 3476869 | 1590565 | 0 | 36 | 12 | 0 | 7484 | 5680 | 1172 | 4612 | 36 | events | 41.8% |
| execute_batch | allowlist | 1 | yes | Tagged | 10 | 6881752 | 2151308 | 11 | 36 | 34 | 1276 | 6144 | 10000 | 4 | 412 | 47 | events | 61.1% |
| request_payouts | allowlist | 1 | yes | Tagged | 20 | 7247812 | 2144405 | 0 | 66 | 22 | 0 | 13844 | 11360 | 2332 | 9212 | 66 | events | 83.6% |
| execute_batch | allowlist | 1 | yes | Tagged | 20 | 15062902 | 3633468 | 21 | 66 | 64 | 2436 | 10824 | 20000 | 4 | 812 | 87 | events | 122.1% |
| request_payouts | allowlist | 1 | yes | Tagged | 40 | 17814132 | 3997285 | 0 | 126 | 42 | 0 | 26564 | 22720 | 4652 | 18412 | 126 | events | 167.1% |
| execute_batch | allowlist | 1 | yes | Tagged | 40 | 37552169 | 8439788 | 41 | 126 | 124 | 4756 | 20184 | 40000 | 4 | 1612 | 167 | events | 244.2% |
| request_payouts | allowlist | 2 | yes | Tagged | 1 | 1053557 | 1310260 | 0 | 10 | 3 | 0 | 1876 | 684 | 128 | 588 | 10 | events | 5.0% |
| execute_batch | allowlist | 2 | yes | Tagged | 1 | 1435899 | 1369638 | 3 | 10 | 8 | 348 | 2048 | 1416 | 4 | 52 | 13 | events | 8.7% |
| request_payouts | allowlist | 2 | yes | Tagged | 10 | 4665963 | 1730695 | 0 | 46 | 12 | 0 | 8644 | 6840 | 1172 | 5772 | 46 | events | 48.9% |
| execute_batch | allowlist | 2 | yes | Tagged | 10 | 9856973 | 2753658 | 21 | 46 | 44 | 2436 | 7304 | 14160 | 4 | 412 | 67 | events | 86.5% |
| request_payouts | allowlist | 2 | yes | Tagged | 20 | 10429161 | 2607865 | 0 | 86 | 22 | 0 | 16164 | 13680 | 2332 | 11532 | 86 | events | 97.7% |
| execute_batch | allowlist | 2 | yes | Tagged | 20 | 23310241 | 5570918 | 41 | 86 | 84 | 4756 | 13144 | 28320 | 4 | 812 | 127 | events | 172.9% |
| request_payouts | allowlist | 2 | yes | Tagged | 40 | 27123329 | 5657005 | 0 | 166 | 42 | 0 | 31204 | 27360 | 4652 | 23052 | 166 | events | 195.4% |
| execute_batch | allowlist | 2 | yes | Tagged | 40 | 62318540 | 15245838 | 81 | 166 | 164 | 9396 | 24824 | 56640 | 4 | 1612 | 247 | events | 345.7% |
| request_payouts | allowlist | 4 | yes | Tagged | 1 | 1277495 | 1322977 | 0 | 12 | 3 | 0 | 2108 | 916 | 128 | 820 | 12 | events | 6.4% |
| execute_batch | allowlist | 4 | yes | Tagged | 1 | 1857906 | 1428443 | 5 | 12 | 10 | 580 | 2280 | 2248 | 4 | 52 | 17 | events | 13.7% |
| request_payouts | allowlist | 4 | yes | Tagged | 10 | 7762704 | 2029495 | 0 | 66 | 12 | 0 | 10964 | 9160 | 1172 | 8092 | 66 | events | 63.1% |
| execute_batch | allowlist | 4 | yes | Tagged | 10 | 17103289 | 4371308 | 41 | 66 | 64 | 4756 | 9624 | 22480 | 4 | 412 | 107 | events | 137.2% |
| request_payouts | allowlist | 4 | yes | Tagged | 20 | 18284936 | 3586865 | 0 | 126 | 22 | 0 | 20804 | 18320 | 2332 | 16172 | 126 | events | 126.0% |
| execute_batch | allowlist | 4 | yes | Tagged | 20 | 44182723 | 11096118 | 81 | 126 | 124 | 9396 | 17784 | 44960 | 4 | 812 | 207 | events | 274.4% |
| request_payouts | allowlist | 4 | yes | Tagged | 40 | 49476689 | 9140605 | 0 | 246 | 42 | 0 | 40484 | 36640 | 4652 | 32332 | 246 | events | 252.0% |
| execute_batch | allowlist | 4 | yes | Tagged | 40 | 129345371 | 35456138 | 161 | 246 | 244 | 18676 | 34104 | 89920 | 4 | 1612 | 407 | events | 548.9% |
| request_payouts | attested | 4 | yes | Tagged | 1 | 10723682 | 1334919 | 0 | 8 | 3 | 0 | 2108 | 916 | 128 | 1380 | 8 | events | 6.4% |
| execute_batch | attested | 4 | yes | Tagged | 1 | 1637676 | 1409137 | 5 | 8 | 10 | 580 | 2280 | 2248 | 4 | 52 | 13 | events | 13.7% |
| request_payouts | attested | 4 | yes | Tagged | 10 | 100887089 | 2020287 | 0 | 26 | 12 | 0 | 10964 | 9160 | 1172 | 13692 | 26 | events | 63.1% |
| execute_batch | attested | 4 | yes | Tagged | 10 | 11598157 | 3671008 | 41 | 26 | 64 | 4756 | 9624 | 22480 | 4 | 412 | 67 | events | 137.2% |
| request_payouts | attested | 4 | yes | Tagged | 20 | 201878448 | 3261937 | 0 | 46 | 22 | 0 | 20804 | 18320 | 2332 | 27372 | 46 | events | 126.0% |
| execute_batch | attested | 4 | yes | Tagged | 20 | 26330567 | 8551598 | 81 | 46 | 124 | 9396 | 17784 | 44960 | 4 | 812 | 127 | events | 274.4% |
| request_payouts | attested | 4 | yes | Tagged | 40 | 406381200 | 7261437 | 0 | 86 | 42 | 0 | 40484 | 36640 | 4652 | 54732 | 86 | events | 252.0% |
| execute_batch | attested | 4 | yes | Tagged | 40 | 67202896 | 25788778 | 161 | 86 | 244 | 18676 | 34104 | 89920 | 4 | 1612 | 247 | events | 548.9% |
| request_payouts | allowlist | 2 | no | Tagged | 1 | 1053557 | 1310260 | 0 | 10 | 3 | 0 | 1876 | 684 | 128 | 588 | 10 | events | 5.0% |
| execute_batch | allowlist | 2 | no | Tagged | 1 | 1305622 | 1349061 | 2 | 10 | 7 | 232 | 1932 | 1176 | 4 | 52 | 12 | events | 7.2% |
| request_payouts | allowlist | 2 | no | Tagged | 10 | 4665963 | 1730695 | 0 | 46 | 12 | 0 | 8644 | 6840 | 1172 | 5772 | 46 | events | 48.9% |
| execute_batch | allowlist | 2 | no | Tagged | 10 | 8222364 | 2404896 | 20 | 46 | 43 | 2320 | 7188 | 11760 | 4 | 412 | 66 | events | 71.8% |
| request_payouts | allowlist | 2 | no | Tagged | 20 | 10429161 | 2607865 | 0 | 86 | 22 | 0 | 16164 | 13680 | 2332 | 11532 | 86 | events | 97.7% |
| execute_batch | allowlist | 2 | no | Tagged | 20 | 19330471 | 4547426 | 40 | 86 | 83 | 4640 | 13028 | 23520 | 4 | 812 | 126 | events | 143.6% |
| request_payouts | allowlist | 2 | no | Tagged | 40 | 27123329 | 5657005 | 0 | 166 | 42 | 0 | 31204 | 27360 | 4652 | 23052 | 166 | events | 195.4% |
| execute_batch | allowlist | 2 | no | Tagged | 40 | 51620247 | 11893686 | 80 | 166 | 163 | 9280 | 24708 | 47040 | 4 | 1612 | 246 | events | 287.1% |

Largest batch that keeps every resource at or under 70 percent of its limit, from a straight line through the 1 and 40 item points (every resource here is linear in the item count):

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

| op | mode | legs | fee | dest | items | instructions | memory | disk reads | mem reads | writes | read bytes | write bytes | event bytes | return bytes | arg bytes | footprint | binding | share |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| execute_batch | allowlist | 16 in all | 0 of 10 | Account | 10 | 6825926 | 2156820 | 16 | 42 | 39 | 1856 | 6724 | 9072 | 4 | 412 | 58 | events | 55.4% |
| execute_batch | allowlist | 16 in all | 0 of 10 | Tagged | 10 | 7038886 | 2172868 | 16 | 42 | 39 | 1856 | 6724 | 10096 | 4 | 412 | 58 | events | 61.6% |
| execute_batch | allowlist | 17 in all | 0 of 10 | Account | 10 | 7109690 | 2211838 | 17 | 43 | 40 | 1972 | 6840 | 9424 | 4 | 412 | 60 | events | 57.5% |
| execute_batch | allowlist | 17 in all | 0 of 10 | Tagged | 10 | 7335960 | 2228889 | 17 | 43 | 40 | 1972 | 6840 | 10512 | 4 | 412 | 60 | events | 64.2% |
| execute_batch | allowlist | 18 in all | 0 of 10 | Account | 10 | 7387480 | 2268180 | 18 | 44 | 41 | 2088 | 6956 | 9776 | 4 | 412 | 62 | events | 59.7% |
| execute_batch | allowlist | 18 in all | 0 of 10 | Tagged | 10 | 7627060 | 2286234 | 18 | 44 | 41 | 2088 | 6956 | 10928 | 4 | 412 | 62 | events | 66.7% |
| execute_batch | allowlist | 19 in all | 0 of 10 | Account | 10 | 7671687 | 2325846 | 19 | 45 | 42 | 2204 | 7072 | 10128 | 4 | 412 | 64 | events | 61.8% |
| execute_batch | allowlist | 19 in all | 0 of 10 | Tagged | 10 | 7924577 | 2344903 | 19 | 45 | 42 | 2204 | 7072 | 11344 | 4 | 412 | 64 | events | 69.3% |
| execute_batch | allowlist | 2 | no | Account | 10 | 7956164 | 2384836 | 20 | 46 | 43 | 2320 | 7188 | 10480 | 4 | 412 | 66 | events | 64.0% |
| execute_batch | allowlist | 2 | no | Tagged | 10 | 8222364 | 2404896 | 20 | 46 | 43 | 2320 | 7188 | 11760 | 4 | 412 | 66 | events | 71.8% |
| execute_batch | allowlist | 1 | no | Account | 10 | 5234573 | 1854491 | 10 | 36 | 33 | 1160 | 6028 | 6960 | 4 | 412 | 46 | events | 42.5% |
| execute_batch | allowlist | 1 | no | Tagged | 10 | 5367673 | 1864521 | 10 | 36 | 33 | 1160 | 6028 | 7600 | 4 | 412 | 46 | events | 46.4% |

The worst cases under the recommended limits (`recommended_limits_keep_headroom`; each is asserted at or under 70 percent of every limit):

| op | mode | legs | fee | dest | items | instructions | memory | disk reads | mem reads | writes | read bytes | write bytes | event bytes | return bytes | arg bytes | footprint | binding | share |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| request_payouts | allowlist | 18 in all | 0 of 10 | Tagged | 10 | 4435463 | 1702669 | 0 | 44 | 12 | 0 | 8412 | 6608 | 1172 | 5540 | 44 | events | 47.5% |
| execute_batch | allowlist | 18 in all | 0 of 10 | Tagged | 10 | 7627060 | 2286234 | 18 | 44 | 41 | 2088 | 6956 | 10928 | 4 | 412 | 62 | events | 66.7% |
| request_payouts | allowlist | 18 in all | 0 of 10 | Account | 10 | 4288297 | 1695389 | 0 | 44 | 12 | 0 | 8340 | 6536 | 1172 | 5468 | 44 | events | 47.0% |
| execute_batch | allowlist | 18 in all | 0 of 10 | Account | 10 | 7387480 | 2268180 | 18 | 44 | 41 | 2088 | 6956 | 9776 | 4 | 412 | 62 | events | 59.7% |
| request_payouts | both | 16 in all | 2 of 4 | Tagged | 4 | 41524928 | 1587905 | 0 | 30 | 6 | 0 | 5060 | 3664 | 476 | 5484 | 30 | events | 25.3% |
| execute_batch | both | 16 in all | 2 of 4 | Tagged | 4 | 5666745 | 2022712 | 17 | 30 | 28 | 1972 | 4728 | 8512 | 4 | 172 | 47 | events | 52.0% |
| request_payouts | both | 4 | yes | Tagged | 3 | 31272104 | 1500015 | 0 | 24 | 5 | 0 | 4076 | 2748 | 360 | 4116 | 24 | events | 19.0% |
| execute_batch | both | 4 | yes | Tagged | 3 | 4450010 | 1827837 | 13 | 24 | 22 | 1508 | 3912 | 6744 | 4 | 132 | 37 | events | 41.2% |
| request_payouts | allowlist | 1 | yes | Tagged | 9 | 3155692 | 1548843 | 0 | 33 | 11 | 0 | 6848 | 5112 | 1056 | 4152 | 33 | events | 37.6% |
| execute_batch | allowlist | 1 | yes | Tagged | 9 | 6121241 | 2036862 | 10 | 33 | 31 | 1160 | 5676 | 9000 | 4 | 372 | 43 | events | 55.0% |
| request_payouts | both | 4 | no | Tagged | 10 | 103819014 | 2285975 | 0 | 66 | 12 | 0 | 10964 | 9160 | 1172 | 13692 | 66 | events | 63.1% |
| request_payouts | attested | 4 | yes | Tagged | 10 | 100887089 | 2020287 | 0 | 26 | 12 | 0 | 10964 | 9160 | 1172 | 13692 | 26 | events | 63.1% |
| request_payouts | both | 4 | no | Account | 10 | 103172734 | 2252695 | 0 | 66 | 12 | 0 | 10804 | 9000 | 1172 | 13532 | 66 | events | 62.1% |

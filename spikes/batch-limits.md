# Batch limits on Soroban and Celo

How many payouts one `request_payouts` or `execute_batch` call can carry, measured instead of guessed, and the value of `MAX_BATCH` that follows. `MAX_BATCH` is a spec constant, identical on both chains, that cannot change after mainnet without a new version.

## Recommendation

| Constant | Value | Applies to |
| --- | --- | --- |
| `MAX_BATCH` | **10** items | `request_payouts`, `execute_batch`, `cancel_batch` |
| `MAX_BATCH_TRANSFERS` (new) | **20** transfers | `execute_batch`: every leg of every item, plus one per item with a non-zero fee |

A batch outside either bound fails with `BATCH_INVALID`, the same code an item count outside `[1, MAX_BATCH]` already gets.

Why two numbers and not one: on Soroban the limit that binds is the per-transaction **contract events size, 16,384 bytes**, which counts the events and the return value. Instructions, memory, entries and bytes are far from their limits. Every SAC transfer emits its own `transfer` event of about 240 bytes that the mandate cannot shrink. So the cost of an execute batch follows the number of transfers, not the number of items. With `MAX_BATCH = 20` alone, a batch of 4-leg payouts with a fee needs 39,844 event bytes, 243 percent of the limit. A single `MAX_BATCH` of 5 would fit the worst case, but it would halve throughput for the common 1-leg payout for no reason.

Headroom under the recommended limits (at least 30 percent required on every resource, both chains):

| Worst case under the limits | Chain | Binding resource | Use | Headroom |
| --- | --- | --- | --- | --- |
| `execute_batch`, 10 items x 2 legs, no fee (20 transfers, all legs) | Soroban, test env | events | 10,484 of 16,384 bytes, 64.0% | 36.0% |
| same, submitted to testnet (tx `149001a6...`, ledger 5244218) | Soroban, testnet | events | 10,564 bytes, 64.5% | 35.5% |
| `request_payouts`, 10 items x 4 legs, BOTH mode | Soroban, test env | events | 10,172 bytes, 62.1% (instructions 103.2M, 25.8%) | 37.9% |
| `execute_batch`, 4 items x (4 legs + fee) | Soroban, test env | events | 7,972 bytes, 48.7% | 51.3% |
| `execute_batch`, 10 items x (1 leg + fee) | Soroban, test env | events | 9,364 bytes, 57.2% | 42.8% |
| `request_payouts`, 10 items x 4 legs, ATTESTED, real USDC | Celo Sepolia `eth_estimateGas` | gas | 3,745,048, 12.5% of 30M, 22.3% of 2^24 | 77.7% |
| `execute_batch`, 10 items x (4 legs + fee), real USDC | Celo Sepolia `eth_estimateGas` | gas | 2,356,198, 7.9% of 30M | 92.1% |

Celo has room for much larger batches, so the shared value is set by Soroban. Both test suites assert these worst cases stay at or under 70 percent of every limit (`recommended_limits_keep_headroom` in Rust, `test_recommended_limits_keep_headroom` in forge), so a protocol or SDK change that eats the margin fails CI.

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

Celo: block gas limit 30,000,000 on Celo Sepolia (`eth_getBlockByNumber`, block 0x25276ac, 2026-10-17; the same on mainnet). Celo runs Prague, which has no separate per-transaction cap. Osaka's EIP-7825 would cap a transaction at 2^24 = 16,777,216 gas, so the limits are also checked against that, in case Celo adopts it.

## What was measured

A prototype of the batch path of the mandate on each chain. Each keeps the real storage layout, check order, events and transfers, and leaves out what a batch does not touch: approvals, cancel, pause and most roles. The prototypes are never deployed outside testnet.

- **Soroban:** `soroban/spikes/batch-limits/proto` holds the contract. Its compiled wasm is committed as `soroban/spikes/batch-limits/fixtures/batch_limits_proto.wasm` (sha256 `03132e3672d80f41...`; rebuild with `stellar contract build --package batch-limits-proto`). `soroban/spikes/batch-limits` loads that wasm into the SDK test env, because the native build meters host functions only and misses most of the cost. The token is a real SAC. Destinations are fresh classic accounts with trustlines, one per leg, so every transfer touches a new trustline; one series uses contract addresses instead. Attestations are real secp256k1 signatures over the 7.3 digest, one per leg. Resources come from the env's cost estimate, plus the XDR size of the return value, because the network counts that against the events limit.
- **Celo:** `celo/test/spikes/BatchLimitsMandate.sol` (prototype) and `BatchLimitsToken.sol` (mock ERC-20 with 6 or 18 decimals) are measured by `celo/test/spikes/BatchLimits.t.sol`. Gas is a `gasleft()` delta around a call that copies no return data, with the mandate and token storage cooled first. Copying return data would charge memory expansion in the test contract, and that cost grows with everything allocated earlier. Transaction gas adds the 21,000 intrinsic and the calldata cost, or the EIP-7623 calldata floor if higher. The figure is about 3,000 high, because the top-level call of a real transaction does not pay the cold account access.
- Batch sizes 1, 10, 20 and 40; 1, 2 and 4 legs; ALLOWLIST, ATTESTED and BOTH modes; a fee on every row except one 1-leg series. Every item is checked to land APPROVED and then EXECUTED.
- Both suites were run twice; the numbers are identical.

```
cd soroban && cargo test -p batch-limits --release -- --nocapture
cd celo && forge test --match-contract BatchLimits -vv
```

## Network cross-checks

Every transaction below was built from a fresh simulation with padded write bytes and judged by `getTransaction`, not by the CLI's summary.

Soroban testnet, the prototype in ATTESTED mode with contract address destinations, protocol 29:

| Call | Result | Network figure | Test env |
| --- | --- | --- | --- |
| `request_payouts` 16 items x 4 legs (tx `f0022483...`, ledger 5244059) | SUCCESS | 16,076 event bytes; 164,785,899 instructions | 16,012 bytes; about 159.9M instructions (straight line through the measured points) |
| `request_payouts` 17 items x 4 legs (tx `b8aa14f5...`, ledger 5244062) | FAILED `resource_limit_exceeded`: "return value pushes events size above network config maximum", 17,080 > 16,384 | | predicted over: 17,012 |
| `execute_batch` 9 items x (4 legs + fee) (tx `bf500256...`, ledger 5244069) | FAILED `resource_limit_exceeded`: "total events size exceeds network config maximum" | 16,616 when it stopped | predicted over: 17,644 |
| `execute_batch` 8 items x (4 legs + fee) (tx `34661070...`, ledger 5244072) | SUCCESS | 15,844 event bytes; 12,678,765 instructions | 15,684 bytes |
| `execute_batch` 10 items x 2 legs to classic accounts (tx `149001a6...`, ledger 5244218) | SUCCESS | 10,564 event bytes; 42 writes; 2,320 read bytes | 10,484 bytes; 43 writes; 2,320 read bytes |

The 17-item request only fails if the return value counts. It does, and the test env model, which adds it, predicts that failure. The env is within 1 percent of the network on event bytes and within 3 percent on instructions; simulation declares about 4 percent more instructions than are used.

Celo Sepolia (chain 11142220), with no funded key. The prototype ran on a local anvil fork of Celo Sepolia, using the real USDC (`0x01C5C0122039549AD1493B8220cABEdD739BC44E`) and USDm (`0xdE9e4C3ce781b4bA68120d6261cbad65ce0aB00b`). Then `eth_estimateGas` ran on the real node, overriding the mandate's code and storage and its token balance slot with that state:

| Call | Celo Sepolia estimate | Fork estimate | Forge, mock token, same shape |
| --- | --- | --- | --- |
| `requestPayouts` 10 x 4 legs, ATTESTED, USDC | 3,745,048 | 3,714,401 | 3,704,893 |
| `executeBatch` 10 x (4 legs + fee), USDC | 2,356,198 | 2,348,417 | 2,049,953 |
| `requestPayouts` 10 x 4 legs, ATTESTED, USDm | 3,745,756 | 3,715,104 | 3,707,173 |
| `executeBatch` 10 x (4 legs + fee), USDm | 2,283,255 | 2,275,648 | 2,049,953 |
| `requestPayouts` 20 x 4 legs, ATTESTED, USDC | 7,416,488 | 7,356,932 | 7,347,148 |
| `executeBatch` 20 x (4 legs + fee), USDC | 4,591,591 | 4,561,400 | 4,015,800 |

Requests agree with forge within about 1 percent. Execute on the real tokens costs 11 to 15 percent more than on the mock token, because the real tokens run more checks per transfer (USDC is a proxy with a blocklist), so execute rows from forge understate the network by that much. Both are linear in the item count.

## Findings beyond the limit

- The return value of `request_payouts` (an `ItemResult` per item, about 116 bytes each) is charged against the same 16,384 bytes as the events. A request batch that fits on events alone can still fail.
- On Soroban, ATTESTED 4-leg requests cost about 10.05M instructions per item, almost all signature recovery. That alone would allow about 27 items with headroom, so instructions are the second constraint after events.
- Fees on testnet are dominated by rent. Extending each payout record for 31 days cost about 0.25 XLM per item: 2.46 of the 2.47 XLM charged for a 10-item request. The first call after deploy also extended the wasm and the instance, and paid 46 XLM. How long records are kept alive, and who pays for it, is a sponsorship cost question.
- Celo calldata stays far above the EIP-7623 floor on every row, so the floor never applies at these sizes.

## Soroban measurements (test env, compiled wasm)

`share` is the binding resource as a share of its limit. "events" counts the events plus the return value, which is what the network charges. `footprint` is the number of distinct keys (disk reads plus in-memory reads). The SDK's own check adds the write entries on top of that, which is stricter than the network rule.

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

## Celo measurements (forge, mock tokens)

`tx gas` is the execution gas plus the intrinsic and calldata gas, or the EIP-7623 floor if higher. The 6 and 18 decimal series are within a few hundred gas of each other; only calldata bytes differ.

| op | mode | legs | fee | decimals | items | execution gas | calldata bytes | tx gas | share of 30M |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| requestPayouts | allowlist | 1 | yes | 18 | 1 | 214211 | 548 | 239167 | 0.7% |
| executeBatch | allowlist | 1 | yes | 18 | 1 | 164882 | 100 | 186726 | 0.6% |
| requestPayouts | allowlist | 1 | yes | 18 | 10 | 1794526 | 4868 | 1852146 | 6.1% |
| executeBatch | allowlist | 1 | yes | 18 | 10 | 1069529 | 388 | 1095969 | 3.6% |
| requestPayouts | allowlist | 1 | yes | 18 | 20 | 3550584 | 9668 | 3644468 | 12.1% |
| executeBatch | allowlist | 1 | yes | 18 | 20 | 2074971 | 708 | 2106507 | 7.0% |
| requestPayouts | allowlist | 1 | yes | 18 | 40 | 7063122 | 19268 | 7229630 | 24.0% |
| executeBatch | allowlist | 1 | yes | 18 | 40 | 4086721 | 1348 | 4128521 | 13.7% |
| requestPayouts | allowlist | 2 | yes | 18 | 1 | 266904 | 676 | 292696 | 0.9% |
| executeBatch | allowlist | 2 | yes | 18 | 1 | 200153 | 100 | 222009 | 0.7% |
| requestPayouts | allowlist | 2 | yes | 18 | 10 | 2321412 | 6148 | 2387212 | 7.9% |
| executeBatch | allowlist | 2 | yes | 18 | 10 | 1422491 | 388 | 1448919 | 4.8% |
| requestPayouts | allowlist | 2 | yes | 18 | 20 | 4604520 | 12228 | 4714944 | 15.7% |
| executeBatch | allowlist | 2 | yes | 18 | 20 | 2781445 | 708 | 2813029 | 9.3% |
| requestPayouts | allowlist | 2 | yes | 18 | 40 | 9171655 | 24388 | 9371027 | 31.2% |
| executeBatch | allowlist | 2 | yes | 18 | 40 | 5501877 | 1348 | 5543605 | 18.4% |
| requestPayouts | allowlist | 4 | yes | 18 | 1 | 374481 | 932 | 401909 | 1.3% |
| executeBatch | allowlist | 4 | yes | 18 | 1 | 270700 | 100 | 292556 | 0.9% |
| requestPayouts | allowlist | 4 | yes | 18 | 10 | 3397147 | 8708 | 3479499 | 11.5% |
| executeBatch | allowlist | 4 | yes | 18 | 10 | 2128850 | 388 | 2155314 | 7.1% |
| requestPayouts | allowlist | 4 | yes | 18 | 20 | 6756512 | 17348 | 6899764 | 22.9% |
| executeBatch | allowlist | 4 | yes | 18 | 20 | 4196114 | 708 | 4227662 | 14.0% |
| requestPayouts | allowlist | 4 | yes | 18 | 40 | 13477720 | 34628 | 13742916 | 45.8% |
| executeBatch | allowlist | 4 | yes | 18 | 40 | 8339048 | 1348 | 8380788 | 27.9% |
| requestPayouts | attested | 1 | yes | 18 | 1 | 222041 | 772 | 248769 | 0.8% |
| executeBatch | attested | 1 | yes | 18 | 1 | 162466 | 100 | 184322 | 0.6% |
| requestPayouts | attested | 1 | yes | 18 | 10 | 1836748 | 7108 | 1912064 | 6.3% |
| executeBatch | attested | 1 | yes | 18 | 10 | 1045329 | 388 | 1071769 | 3.5% |
| requestPayouts | attested | 1 | yes | 18 | 20 | 3630980 | 14148 | 3760352 | 12.5% |
| executeBatch | attested | 1 | yes | 18 | 20 | 2026475 | 708 | 2058023 | 6.8% |
| requestPayouts | attested | 1 | yes | 18 | 40 | 7220116 | 28228 | 7457300 | 24.8% |
| executeBatch | attested | 1 | yes | 18 | 40 | 3989342 | 1348 | 4031070 | 13.4% |
| requestPayouts | attested | 2 | yes | 18 | 1 | 278550 | 1124 | 307874 | 1.0% |
| executeBatch | attested | 2 | yes | 18 | 1 | 195049 | 100 | 216905 | 0.7% |
| requestPayouts | attested | 2 | yes | 18 | 10 | 2401950 | 10628 | 2503130 | 8.3% |
| executeBatch | attested | 2 | yes | 18 | 10 | 1371307 | 388 | 1397735 | 4.6% |
| requestPayouts | attested | 2 | yes | 18 | 20 | 4761522 | 21188 | 4942454 | 16.4% |
| executeBatch | attested | 2 | yes | 18 | 20 | 2678754 | 708 | 2710338 | 9.0% |
| requestPayouts | attested | 2 | yes | 18 | 40 | 9482312 | 42308 | 9822352 | 32.7% |
| executeBatch | attested | 2 | yes | 18 | 40 | 5295200 | 1348 | 5336940 | 17.7% |
| requestPayouts | attested | 4 | yes | 18 | 1 | 393822 | 1828 | 428338 | 1.4% |
| executeBatch | attested | 4 | yes | 18 | 1 | 260217 | 100 | 282073 | 0.9% |
| requestPayouts | attested | 4 | yes | 18 | 10 | 3554313 | 17668 | 3707173 | 12.3% |
| executeBatch | attested | 4 | yes | 18 | 10 | 2023501 | 388 | 2049953 | 6.8% |
| requestPayouts | attested | 4 | yes | 18 | 20 | 7067224 | 35268 | 7351708 | 24.5% |
| executeBatch | attested | 4 | yes | 18 | 20 | 3984252 | 708 | 4015800 | 13.3% |
| requestPayouts | attested | 4 | yes | 18 | 40 | 14096508 | 70468 | 14644096 | 48.8% |
| executeBatch | attested | 4 | yes | 18 | 40 | 7910658 | 1348 | 7952386 | 26.5% |
| requestPayouts | both | 1 | yes | 18 | 1 | 225036 | 772 | 251764 | 0.8% |
| executeBatch | both | 1 | yes | 18 | 1 | 165154 | 100 | 187010 | 0.6% |
| requestPayouts | both | 1 | yes | 18 | 10 | 1866676 | 7108 | 1942040 | 6.4% |
| executeBatch | both | 1 | yes | 18 | 10 | 1072249 | 388 | 1098701 | 3.6% |
| requestPayouts | both | 1 | yes | 18 | 20 | 3691130 | 14148 | 3820466 | 12.7% |
| executeBatch | both | 1 | yes | 18 | 20 | 2080411 | 708 | 2111959 | 7.0% |
| requestPayouts | both | 1 | yes | 18 | 40 | 7340507 | 28228 | 7577643 | 25.2% |
| executeBatch | both | 1 | yes | 18 | 40 | 4097601 | 1348 | 4139341 | 13.7% |
| requestPayouts | both | 2 | yes | 18 | 1 | 284541 | 1124 | 313853 | 1.0% |
| executeBatch | both | 2 | yes | 18 | 1 | 200425 | 100 | 222281 | 0.7% |
| requestPayouts | both | 2 | yes | 18 | 10 | 2461860 | 10628 | 2563052 | 8.5% |
| executeBatch | both | 2 | yes | 18 | 10 | 1425211 | 388 | 1451675 | 4.8% |
| requestPayouts | both | 2 | yes | 18 | 20 | 4882016 | 21188 | 5062984 | 16.8% |
| executeBatch | both | 2 | yes | 18 | 20 | 2786885 | 708 | 2818433 | 9.3% |
| requestPayouts | both | 2 | yes | 18 | 40 | 9724176 | 42308 | 10064324 | 33.5% |
| executeBatch | both | 2 | yes | 18 | 40 | 5512757 | 1348 | 5554533 | 18.5% |
| requestPayouts | both | 4 | yes | 18 | 1 | 405785 | 1828 | 440301 | 1.4% |
| executeBatch | both | 4 | yes | 18 | 1 | 270972 | 100 | 292828 | 0.9% |
| requestPayouts | both | 4 | yes | 18 | 10 | 3674570 | 17668 | 3827502 | 12.7% |
| executeBatch | both | 4 | yes | 18 | 10 | 2131570 | 388 | 2158022 | 7.1% |
| requestPayouts | both | 4 | yes | 18 | 20 | 7308756 | 35268 | 7593180 | 25.3% |
| executeBatch | both | 4 | yes | 18 | 20 | 4201554 | 708 | 4233126 | 14.1% |
| requestPayouts | both | 4 | yes | 18 | 40 | 14583749 | 70468 | 15131493 | 50.4% |
| executeBatch | both | 4 | yes | 18 | 40 | 8349928 | 1348 | 8391668 | 27.9% |
| requestPayouts | allowlist | 1 | yes | 6 | 1 | 214211 | 548 | 239059 | 0.7% |
| executeBatch | allowlist | 1 | yes | 6 | 1 | 164882 | 100 | 186726 | 0.6% |
| requestPayouts | allowlist | 1 | yes | 6 | 10 | 1794526 | 4868 | 1851066 | 6.1% |
| executeBatch | allowlist | 1 | yes | 6 | 10 | 1069529 | 388 | 1095969 | 3.6% |
| requestPayouts | allowlist | 1 | yes | 6 | 20 | 3550584 | 9668 | 3642308 | 12.1% |
| executeBatch | allowlist | 1 | yes | 6 | 20 | 2074971 | 708 | 2106507 | 7.0% |
| requestPayouts | allowlist | 1 | yes | 6 | 40 | 7063122 | 19268 | 7225310 | 24.0% |
| executeBatch | allowlist | 1 | yes | 6 | 40 | 4086721 | 1348 | 4128521 | 13.7% |
| requestPayouts | allowlist | 2 | yes | 6 | 1 | 266904 | 676 | 292552 | 0.9% |
| executeBatch | allowlist | 2 | yes | 6 | 1 | 200153 | 100 | 222009 | 0.7% |
| requestPayouts | allowlist | 2 | yes | 6 | 10 | 2321412 | 6148 | 2385772 | 7.9% |
| executeBatch | allowlist | 2 | yes | 6 | 10 | 1422491 | 388 | 1448919 | 4.8% |
| requestPayouts | allowlist | 2 | yes | 6 | 20 | 4604520 | 12228 | 4712064 | 15.7% |
| executeBatch | allowlist | 2 | yes | 6 | 20 | 2781445 | 708 | 2813029 | 9.3% |
| requestPayouts | allowlist | 2 | yes | 6 | 40 | 9171655 | 24388 | 9365267 | 31.2% |
| executeBatch | allowlist | 2 | yes | 6 | 40 | 5501877 | 1348 | 5543605 | 18.4% |
| requestPayouts | allowlist | 4 | yes | 6 | 1 | 374481 | 932 | 401681 | 1.3% |
| executeBatch | allowlist | 4 | yes | 6 | 1 | 270700 | 100 | 292556 | 0.9% |
| requestPayouts | allowlist | 4 | yes | 6 | 10 | 3397147 | 8708 | 3477219 | 11.5% |
| executeBatch | allowlist | 4 | yes | 6 | 10 | 2128850 | 388 | 2155314 | 7.1% |
| requestPayouts | allowlist | 4 | yes | 6 | 20 | 6756512 | 17348 | 6895204 | 22.9% |
| executeBatch | allowlist | 4 | yes | 6 | 20 | 4196114 | 708 | 4227662 | 14.0% |
| requestPayouts | allowlist | 4 | yes | 6 | 40 | 13477720 | 34628 | 13733796 | 45.7% |
| executeBatch | allowlist | 4 | yes | 6 | 40 | 8339048 | 1348 | 8380788 | 27.9% |
| requestPayouts | attested | 1 | yes | 6 | 1 | 222041 | 772 | 248661 | 0.8% |
| executeBatch | attested | 1 | yes | 6 | 1 | 162466 | 100 | 184322 | 0.6% |
| requestPayouts | attested | 1 | yes | 6 | 10 | 1836748 | 7108 | 1910984 | 6.3% |
| executeBatch | attested | 1 | yes | 6 | 10 | 1045329 | 388 | 1071769 | 3.5% |
| requestPayouts | attested | 1 | yes | 6 | 20 | 3630980 | 14148 | 3758192 | 12.5% |
| executeBatch | attested | 1 | yes | 6 | 20 | 2026475 | 708 | 2058023 | 6.8% |
| requestPayouts | attested | 1 | yes | 6 | 40 | 7220116 | 28228 | 7452980 | 24.8% |
| executeBatch | attested | 1 | yes | 6 | 40 | 3989342 | 1348 | 4031070 | 13.4% |
| requestPayouts | attested | 2 | yes | 6 | 1 | 278550 | 1124 | 307730 | 1.0% |
| executeBatch | attested | 2 | yes | 6 | 1 | 195049 | 100 | 216905 | 0.7% |
| requestPayouts | attested | 2 | yes | 6 | 10 | 2401950 | 10628 | 2501690 | 8.3% |
| executeBatch | attested | 2 | yes | 6 | 10 | 1371307 | 388 | 1397735 | 4.6% |
| requestPayouts | attested | 2 | yes | 6 | 20 | 4761522 | 21188 | 4939574 | 16.4% |
| executeBatch | attested | 2 | yes | 6 | 20 | 2678754 | 708 | 2710338 | 9.0% |
| requestPayouts | attested | 2 | yes | 6 | 40 | 9482312 | 42308 | 9816592 | 32.7% |
| executeBatch | attested | 2 | yes | 6 | 40 | 5295200 | 1348 | 5336940 | 17.7% |
| requestPayouts | attested | 4 | yes | 6 | 1 | 393822 | 1828 | 428110 | 1.4% |
| executeBatch | attested | 4 | yes | 6 | 1 | 260217 | 100 | 282073 | 0.9% |
| requestPayouts | attested | 4 | yes | 6 | 10 | 3554313 | 17668 | 3704893 | 12.3% |
| executeBatch | attested | 4 | yes | 6 | 10 | 2023501 | 388 | 2049953 | 6.8% |
| requestPayouts | attested | 4 | yes | 6 | 20 | 7067224 | 35268 | 7347148 | 24.4% |
| executeBatch | attested | 4 | yes | 6 | 20 | 3984252 | 708 | 4015800 | 13.3% |
| requestPayouts | attested | 4 | yes | 6 | 40 | 14096508 | 70468 | 14634976 | 48.7% |
| executeBatch | attested | 4 | yes | 6 | 40 | 7910658 | 1348 | 7952386 | 26.5% |
| requestPayouts | both | 1 | yes | 6 | 1 | 225036 | 772 | 251656 | 0.8% |
| executeBatch | both | 1 | yes | 6 | 1 | 165154 | 100 | 187010 | 0.6% |
| requestPayouts | both | 1 | yes | 6 | 10 | 1866676 | 7108 | 1940960 | 6.4% |
| executeBatch | both | 1 | yes | 6 | 10 | 1072249 | 388 | 1098701 | 3.6% |
| requestPayouts | both | 1 | yes | 6 | 20 | 3691130 | 14148 | 3818306 | 12.7% |
| executeBatch | both | 1 | yes | 6 | 20 | 2080411 | 708 | 2111959 | 7.0% |
| requestPayouts | both | 1 | yes | 6 | 40 | 7340507 | 28228 | 7573323 | 25.2% |
| executeBatch | both | 1 | yes | 6 | 40 | 4097601 | 1348 | 4139341 | 13.7% |
| requestPayouts | both | 2 | yes | 6 | 1 | 284541 | 1124 | 313709 | 1.0% |
| executeBatch | both | 2 | yes | 6 | 1 | 200425 | 100 | 222281 | 0.7% |
| requestPayouts | both | 2 | yes | 6 | 10 | 2461860 | 10628 | 2561612 | 8.5% |
| executeBatch | both | 2 | yes | 6 | 10 | 1425211 | 388 | 1451675 | 4.8% |
| requestPayouts | both | 2 | yes | 6 | 20 | 4882016 | 21188 | 5060104 | 16.8% |
| executeBatch | both | 2 | yes | 6 | 20 | 2786885 | 708 | 2818433 | 9.3% |
| requestPayouts | both | 2 | yes | 6 | 40 | 9724176 | 42308 | 10058564 | 33.5% |
| executeBatch | both | 2 | yes | 6 | 40 | 5512757 | 1348 | 5554533 | 18.5% |
| requestPayouts | both | 4 | yes | 6 | 1 | 405785 | 1828 | 440073 | 1.4% |
| executeBatch | both | 4 | yes | 6 | 1 | 270972 | 100 | 292828 | 0.9% |
| requestPayouts | both | 4 | yes | 6 | 10 | 3674570 | 17668 | 3825222 | 12.7% |
| executeBatch | both | 4 | yes | 6 | 10 | 2131570 | 388 | 2158022 | 7.1% |
| requestPayouts | both | 4 | yes | 6 | 20 | 7308756 | 35268 | 7588620 | 25.2% |
| executeBatch | both | 4 | yes | 6 | 20 | 4201554 | 708 | 4233126 | 14.1% |
| requestPayouts | both | 4 | yes | 6 | 40 | 14583749 | 70468 | 15122373 | 50.4% |
| executeBatch | both | 4 | yes | 6 | 40 | 8349928 | 1348 | 8391668 | 27.9% |
| requestPayouts | allowlist | 1 | no | 6 | 1 | 214211 | 548 | 239035 | 0.7% |
| executeBatch | allowlist | 1 | no | 6 | 1 | 137062 | 100 | 158906 | 0.5% |
| requestPayouts | allowlist | 1 | no | 6 | 10 | 1794526 | 4868 | 1850826 | 6.1% |
| executeBatch | allowlist | 1 | no | 6 | 10 | 1006275 | 388 | 1032715 | 3.4% |
| requestPayouts | allowlist | 1 | no | 6 | 20 | 3550584 | 9668 | 3641828 | 12.1% |
| executeBatch | allowlist | 1 | no | 6 | 20 | 1972244 | 708 | 2003780 | 6.6% |
| requestPayouts | allowlist | 1 | no | 6 | 40 | 7063122 | 19268 | 7224350 | 24.0% |
| executeBatch | allowlist | 1 | no | 6 | 40 | 3904756 | 1348 | 3946556 | 13.1% |
| requestPayouts | both | 4 | yes | 18 | 10 | 3674510 | 17668 | 3827418 | 12.7% |

Largest batch with 30 percent headroom, against the 30M block gas limit and against the 2^24 Osaka transaction cap, fee on:

| mode | legs | decimals | requestPayouts | executeBatch |
| --- | --- | --- | --- | --- |
| allowlist | 1 | 18 | 116 / 65 | 206 / 115 |
| allowlist | 2 | 18 | 89 / 50 | 153 / 85 |
| allowlist | 4 | 18 | 61 / 34 | 100 / 56 |
| attested | 1 | 18 | 113 / 63 | 212 / 118 |
| attested | 2 | 18 | 85 / 47 | 159 / 88 |
| attested | 4 | 18 | 57 / 32 | 106 / 59 |
| both | 1 | 18 | 111 / 62 | 206 / 115 |
| both | 2 | 18 | 83 / 46 | 152 / 85 |
| both | 4 | 18 | 55 / 31 | 100 / 56 |
| allowlist | 1 | 6 | 116 / 65 | 206 / 115 |
| allowlist | 2 | 6 | 90 / 50 | 153 / 85 |
| allowlist | 4 | 6 | 61 / 34 | 100 / 56 |
| attested | 1 | 6 | 113 / 63 | 212 / 118 |
| attested | 2 | 6 | 85 / 47 | 159 / 88 |
| attested | 4 | 6 | 57 / 32 | 106 / 59 |
| both | 1 | 6 | 111 / 62 | 206 / 115 |
| both | 2 | 6 | 83 / 46 | 152 / 85 |
| both | 4 | 6 | 55 / 31 | 100 / 56 |

# Encodings

Byte layouts that the mandate, the off-chain services and the test vectors must produce identically. `‖` is byte concatenation, `u8`, `u16be`, `u32be` and `u64be` are unsigned big-endian integers of 1, 2, 4 and 8 bytes, and `[n]` is a fixed width of `n` bytes. Every variable-length field carries a length prefix unless it is the last field, so no two field sequences encode to the same bytes.

Every byte count and length below is derived from the layout, never copied. `scripts/attestation-message.py --derive` computes each one from bytes it builds, and `scripts/attestation-message.py --check-doc spec/ENCODING.md` fails if a figure here disagrees with it, if a figure is reworded out of reach of its check, or if a new figure is added with no derivation behind it. `--check-constants spec/constants.json` does the same for the attestation lengths there.

So that the check covers this file as it will be written and not only as it reads today, every length here is digits followed by `byte` or `bytes`: never spelled in words, never given in bits. A fixed width inside a layout block is written `name[n]`, and in a three-column table a bare number in the last column is read as a byte length, so any other count carries its unit.

## IDs

`payout_id`, `recipient_ref`, `task_ref` and `tenant_ref` are 32 bytes. The contract only stores and compares them (`MANDATE-PAYOUT-008`); they are derived off chain. `leg_key` is 33 bytes, the `payout_id` and one byte of leg index; it is used only off chain, for delivery steps after settlement.

```
payout_id     = SHA-256("ujira/payout/v1"    ‖ principal_uuid[16] ‖ project_uuid[16]
                        ‖ u16be(len(idempotency_key)) ‖ idempotency_key_utf8 ‖ u32be(attempt))
recipient_ref = SHA-256("ujira/recipient/v1" ‖ principal_uuid[16] ‖ recipient_uuid[16])
task_ref      = SHA-256("ujira/task/v1"      ‖ project_uuid[16] ‖ task_id_utf8)
tenant_ref    = SHA-256("ujira/tenant/v1"    ‖ principal_uuid[16] ‖ project_uuid[16])
leg_key       = payout_id[32] ‖ u8(leg_index)
```

- A UUID is its 16 bytes in RFC 9562 order. Domain tags are ASCII without a terminator.
- `attempt` starts at 0; a new attempt for the same idempotency key needs the previous attempt provably dead (`CHAINOPS-XCHAIN-001`).
- `task_id_utf8` is the last field, so it needs no length prefix.
- `recipient_ref` is salted by principal, so one person has unrelated refs under different principals.

## Addresses

An address in a hashed layout is its `Address::to_xdr` bytes: the XDR of an `ScVal` of type `SCV_ADDRESS` (18).

| Address | Bytes | Length |
| --- | --- | --- |
| Account `G` | `u32be(18) ‖ u32be(0) ‖ u32be(0) ‖ ed25519_key[32]` (`SC_ADDRESS_TYPE_ACCOUNT`, `PUBLIC_KEY_TYPE_ED25519`) | 44 |
| Contract `C` | `u32be(18) ‖ u32be(1) ‖ contract_id[32]` (`SC_ADDRESS_TYPE_CONTRACT`) | 40 |

A muxed `M` address is never hashed as such: its base `G` address is encoded as above and its id is the tag (`MANDATE-RECIPIENT-001`).

## Attestation message (version 2)

Recipient attestations are ed25519 signatures by the attester (`MANDATE-RECIPIENT-012`).

```
fields  = network_id[32]
        ‖ u8(len(mandate)) ‖ mandate
        ‖ recipient_ref[32]
        ‖ country_policy_hash[32]
        ‖ u64be(policy_epoch)
        ‖ u64be(valid_until)
        ‖ u8(len(destination)) ‖ destination
        ‖ u8(has_tag) ‖ u64be(tag)
message = "UJIRA/ATTEST/V2" ‖ SHA-256(fields)
```

- `network_id` is the Stellar network ID, the SHA-256 of the network passphrase; the contract reads it from its environment.
- `mandate` is the mandate's own address and `destination` the leg's base address, both as `Address::to_xdr` bytes (40 bytes for a contract, 44 for an account), each after a one-byte length.
- `policy_epoch` is the mandate's policy epoch the attestation is issued under (`MANDATE-RECIPIENT-019`), encoded as `u64be`, 8 bytes, right after `country_policy_hash`. The mandate stores it as a `u64` too (`MANDATE-RECIPIENT-017`), so the encoded value is exactly the stored one.
- `has_tag` is 0 or 1. With no tag, `has_tag = 0` and `tag = 0`, so an attestation for an address never covers a sub-account of it, and a tag of 0 is distinct from no tag.
- `message` is exactly `ATTESTATION_MESSAGE_BYTES` = 47 bytes: the 15 ASCII bytes of the prefix and the 32-byte SHA-256. It can never equal a 32-byte transaction hash, and the prefix sets it apart from any other UJIRA signature.
- The amount and the fee are not attested.
- The attester key is a 32-byte ed25519 public key (`ATTESTER_KEY_BYTES`); 32 zero bytes are refused (`MANDATE-CONFIG-005`), and whether the key is a canonical point that is not of small order is checked off chain (`CHAINOPS-CREATE-005`).
- The signature is exactly 64 bytes (`ATTESTATION_SIGNATURE_BYTES`), verified strictly by the host; any failure traps (`MANDATE-RECIPIENT-012`).

Versions: version 1, never deployed, used another signature scheme under the prefix `UJIRA/ATTEST/V1`. A future scheme, for example an EVM signature scheme on another chain, takes its own prefix such as `UJIRA/ATTEST/V3` and never reuses `UJIRA/ATTEST/V2`.

### Worked example

Inputs (the `valid_untagged` case of the attestation parity fixture):

| Field | Value |
| --- | --- |
| Network passphrase | `Test SDF Network ; September 2015` |
| `network_id` | `cee0302d59844d32bdca915c8203dd44b33fbb7edc19051ea37abedf28ecd472` |
| Mandate | `CDUKSTCG3XKTPTHMNRJZIW23J3T5DKHASDPQ3CIRKQ4YJH3PDOFFULZA`, bytes `0000001200000001e8a94c46ddd537ccec6c53945b5b4ee7d1a8e090df0d89115439849f6f1b8a5a` |
| `recipient_ref` | `187204dacf1b6ae9830406f0405b3710d69d86df034cf3fe3753e7b460c8e97d` |
| `country_policy_hash` | `343fb1d75ee948735be1a836a29a40ecd0612986ab9bd1183f8271ccca830415` |
| `policy_epoch` | 0 |
| `valid_until` | 1767229200 |
| Destination | `GDTYAUMMX45E2SU2Q7JRJHCXDIYTEUAXVBHZ7DVCVXZGMAKMWFL7SOY4`, untagged, bytes `000000120000000000000000e780518cbf3a4d4a9a87d3149c571a31325017a84f9f8ea2adf266014cb157f9` |

Results:

| Output | Value |
| --- | --- |
| `fields` (207 bytes) | `cee0302d59844d32bdca915c8203dd44b33fbb7edc19051ea37abedf28ecd472280000001200000001e8a94c46ddd537ccec6c53945b5b4ee7d1a8e090df0d89115439849f6f1b8a5a187204dacf1b6ae9830406f0405b3710d69d86df034cf3fe3753e7b460c8e97d343fb1d75ee948735be1a836a29a40ecd0612986ab9bd1183f8271ccca8304150000000000000000000000006955c7102c000000120000000000000000e780518cbf3a4d4a9a87d3149c571a31325017a84f9f8ea2adf266014cb157f9000000000000000000` |
| `SHA-256(fields)` | `6786dd0154eaa55fca26d77da8952c91df2fcc0927e381771667f4c37a7f115e` |
| `message` (47 bytes) | `554a4952412f4154544553542f56326786dd0154eaa55fca26d77da8952c91df2fcc0927e381771667f4c37a7f115e` |
| Attester public key | `421adb50c10fe73fb1fc0d4211a16911a188bb001eaf0391b479316026141d48` |
| Signature | `043dcca5e9a6dfd41320f9ecf7b4778bf00ff706ac26d865f4c1ff6366a49e0497e63ef43e2a0e6404218231d10d11aa030f954d9eb799ff92f0328e1cae2208` |

The attester key is a test key, public by construction: its ed25519 seed is the SHA-256 of the ASCII label `UJIRA/TEST-ONLY/S18/attester-ed25519`, as in the fixture. It must never be configured as an attester outside tests. ed25519 signatures are deterministic (RFC 8032), so anyone can rebuild the signature from that seed and the message. `scripts/attestation-message.py --example` recomputes `fields`, the SHA-256 and the message from the inputs above, without the Stellar SDK, and `scripts/attestation-message.test.sh` checks that it prints exactly the values in this file. The `valid_untagged` case of the attestation parity fixture carries the same bytes: its fields, SHA-256, message, public key and signature are identical.

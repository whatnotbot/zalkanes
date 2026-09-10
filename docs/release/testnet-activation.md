# Zalkanes Public Testnet Activation — Milestone 2

Status: in progress (provisioning + acceptance in flight)

## Frozen consensus parameters

| Field | Value |
|-------|-------|
| **TESTNET_ACTIVATION_HEIGHT** | **4,338,100** |
| **TESTNET_ACTIVATION_BLOCK_HASH** | **`000036528e38f12ab5ab5dba462f966496178a4a2138ff4f9acc6fb09d3e8098`** |
| **MAINNET_ACTIVATION_HEIGHT** | `None` (pre-audit — MUST NOT be set) |
| Protocol version | 0 |
| Active network upgrade | Nu6.3 (Ironwood) |
| Consensus branch id | `0x37A5165B` |
| Zebra | v6.3.0 (own full validator, trust anchor) |
| Wasmi | `=2.0.0` |
| librustzcash | zcash_primitives 0.30.1, zcash_protocol 0.10.6, zcash_transparent 0.10.0, zcash_script 0.4.3 |
| Carrier spec | ADR-0003 (P2SH carrier, 36-byte redeem script, ≤520-byte pushes) |
| Git commit (activation frozen) | `612f9b8` |

## Activation rationale

`TESTNET_ACTIVATION_HEIGHT = 4,338,100` was frozen immediately before the
controlled first testnet deployment, with the external public testnet tip at
4,338,016 blocks (branch id `0x37A5165B` = Nu6.3). The height is just above the
tip so Zalkanes never interprets arbitrary pre-activation testnet history as
protocol messages. Zalkanes testnet state begins **empty** at activation; blocks
below the activation height are fast-forwarded deterministically (state root is
the empty root).

## Funding

| Field | Value |
|-------|-------|
| Recipient transparent address | `tmHvTQRfAw5uKq1j4M8gZH4JQJ3J6XWmi9X` |
| Faucet | Jino Labs self-sovereign faucet (`zcashfaucet.jinolabs.xyz`), shielded→transparent z→t deshield |
| Full funding txid | `c096d9a6c188e9c235e02f789463b8205e2d9331396e0d4dcd34800329fe4bf2` |
| vout | 0 |
| value | 0.1 TAZ (10,000,000 zatoshi) |
| scriptPubKey | `76a9145a0b114d8e72b3319a9c14d9cd307912b5b3545788ac` (P2PKH) |
| block height / hash / confirmations | 4,338,011 / `005c24cf081eee44ac8940dc8dc6c36b0598ad830df61cf6218ebb547b9914be` / 145+ (verified through our Zebra) |

## Acceptance evidence

*(filled in as the acceptance sequence executes: PREPARE → DEPLOY → CALL×2 →
view get()==2 → restart persistence → fresh reindex identical state root.)*

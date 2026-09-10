# Zalkanes Public Testnet Activation — Milestone 2

Status: **complete** (all acceptance evidence verified)

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

| Step | Value |
|------|-------|
| **PREPARE txid** | `5017d2331b77101eb7cf8630903c9a5fef45bafb3dceffac2e56b5b30864d5fe` |
| PREPARE height / block hash / fee | 4,338,159 / `007cd636b357736d174a5081c8011e0897f3bdcd8484dad107c5b13e5e67592b` / 15,000 zat |
| **DEPLOY wasm size / code hash** | 2513 B / `fa8289fbc0fdb132e57f939033a9971b0ee2990ec49db247aad3f682aa804cc7` |
| **DEPLOY txid** | `d6e8552c3622106382f71c8285e35565fd4451fe3bf5c2ca87d7fd60626e2e26` |
| DEPLOY height / block hash / fee | 4,338,160 / `006ad6137b69aa784b5f0d8bddc6dff49e827ca5c7b72752b34ce7b555a451f7` / 95,000 zat |
| **Contract id** | `607a6246c512239a23f51cf8053444d4d76e7684c1a53dc26a626b474a8cf3c0` |
| reconstructed SHA256 == code hash | **yes** |
| CLI ContractId == indexed ContractId | **yes** |
| **CALL #1 txid** | `7a8bbf7df470c9a41a47314e00c55605e910b80c703346ee233aa49e966ea094` |
| CALL #1 height / success / fuel / root | 4,338,164 / true / 3936 / `fce484adb8a759bddebe137278fb2b704d34b49eda73a210d38987b6d040a0f9` |
| **CALL #2 txid** | `933eb98929c28678a57c54c73fba36b279ae2ad9098aa6fc6edb23fbc5450f0d` |
| CALL #2 height / success / fuel / root | 4,338,166 / true / 3936 / `8d699bab97b280d7008c59f27c1250f05de42503a0354282b850616e0344c625` |
| **view get()** | **2** |
| root at activation (empty) | `b120099c167da673588b15aa827c3bbd9339a9a2d934b0d20c99cebe69f8ffe6` |
| root after deploy | `b5b20286d961283bfbbecc758f4bac1b9f06da0373b107094e84aa32a057aa30` |
| root after call #1 | `fce484adb8a759bddebe137278fb2b704d34b49eda73a210d38987b6d040a0f9` |
| root after call #2 | `8d699bab97b280d7008c59f27c1250f05de42503a0354282b850616e0344c625` |
| **Persistence** (restart) | root unchanged `8d699bab…`, `get() == 2`, code hash identical |
| **Fresh reindex** | final root `8d699bab…` identical, ContractId identical, `get() == 2` |
| **Public RPC** | `https://zalkanes-testnet-production.up.railway.app` — network `test`, syncing `false` |

Distinctness checks: `root_after_deploy != root_at_activation`,
`root_after_call_1 != root_after_deploy`, `root_after_call_2 != root_after_call_1`
— all hold.

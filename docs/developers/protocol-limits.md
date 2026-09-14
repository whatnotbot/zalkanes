# Protocol limits

Every consensus constant lives in `protocol/v0.toml`, whose SHA-256 is the
protocol manifest hash that every node advertises in `zalkanes_getInfo`. The
table below is **generated from that file** by `scripts/check-docs.sh --fix`
and CI fails if it drifts; the Rust constants in
`crates/zalkanes-core/src/consensus.rs` are checked against the same values.
Do not edit the numbers by hand.

<!-- generated:protocol-limits:begin -->
Source: `protocol/v0.toml`, SHA-256 `06e3df62e5e98a3c276b583e038cbea8d05934e9d2a8e7c00299fec3140bf4bb`.

| section | key | value |
|---|---|---|
| `limits` | `max_code_bytes` | `262144` |
| `limits` | `max_chunks` | `188` |
| `limits` | `max_call_inline_bytes` | `38` |
| `limits` | `max_call_input_bytes` | `65536` |
| `limits` | `max_call_carrier_chunks` | `47` |
| `limits` | `max_return_data_bytes` | `65536` |
| `limits` | `max_storage_key_bytes` | `256` |
| `limits` | `max_storage_value_bytes` | `65536` |
| `limits` | `max_storage_writes_per_call` | `256` |
| `limits` | `max_fuel_per_call` | `10000000` |
| `limits` | `max_fuel_per_zcash_tx` | `100000000` |
| `limits` | `max_fuel_per_zcash_block` | `1000000000` |
| `limits` | `max_zalk_messages_per_tx` | `16` |
| `limits` | `max_zalk_messages_per_block` | `4096` |
| `limits` | `max_carrier_bytes_per_block` | `4194304` |
| `limits` | `max_deploy_bytes_per_block` | `4194304` |
| `limits` | `max_call_depth` | `16` |
| `limits` | `max_linear_memory_pages` | `256` |
| `limits` | `max_table_elements` | `16384` |
| `limits` | `max_globals` | `512` |
| `limits` | `max_functions` | `4096` |
| `limits` | `max_imports` | `64` |
| `limits` | `max_exports` | `64` |
| `carrier` | `redeem_script_hex` | `21{pubkey33}ac61` |
| `carrier` | `redeem_script_len` | `36` |
| `carrier` | `max_push_size` | `520` |
| `carrier` | `max_standard_scriptsig_size` | `1650` |
| `carrier` | `chunk_payload_size` | `1400` |
| `wasm` | `engine` | `wasmi` |
| `wasm` | `engine_version` | `2.0.0` |
| `wasm` | `floating_point` | `false` |
| `wasm` | `memory64` | `false` |
| `wasm` | `simd` | `false` |
| `wasm` | `threads` | `false` |
| `wasm` | `metered_fuel` | `true` |
| `network` | `mainnet_activation_height` | `None` |
| `network` | `testnet_activation_height` | `4346500` |
| `network` | `regtest_activation_height` | `1` |
<!-- generated:protocol-limits:end -->

## What the limits mean for a contract author

Module structure (checked at deploy; a failing module is rejected and never
executable):

- `max_code_bytes`: size of the `.wasm` file. Also the largest size the
  carrier can deliver (`max_chunks` chunks of `chunk_payload_size` bytes).
- `max_functions`, `max_globals`, `max_imports`, `max_exports`,
  `max_table_elements`: counted over the whole module, imported and defined
  entities alike.
- `max_linear_memory_pages`: 64 KiB pages; growth past this fails inside WASM.

Execution (per call):

- `max_fuel_per_call`: the budget one CALL or VIEW starts with. A CALL may
  get less if the transaction or block budgets (`max_fuel_per_zcash_tx`,
  `max_fuel_per_zcash_block`) are nearly spent by earlier calls.
- `max_return_data_bytes`: the largest output `output_write` accepts.
- `max_storage_key_bytes`, `max_storage_value_bytes`: per key and per value.
- `max_storage_writes_per_call`: distinct keys set or deleted in one call.
- `max_call_depth`: exists in the manifest; v0 has no nested calls, so it is
  never reached.

Transactions and blocks (enforced by the indexer):

- `max_call_inline_bytes`: input that fits in the OP_RETURN, which is what the
  CLI sends. `max_call_input_bytes` and `max_call_carrier_chunks` bound the
  carrier-delivered CALL that the protocol defines and the CLI does not yet
  build.
- `max_zalk_messages_per_tx`, `max_zalk_messages_per_block`: messages beyond
  these are ignored.
- `max_carrier_bytes_per_block`, `max_deploy_bytes_per_block`: total carrier
  data a block may deliver; excess deploys in a block are ignored.

Carrier: `chunk_payload_size` bytes of WASM per DEPLOY input, pushed in pieces
of at most `max_push_size` bytes, with the whole scriptSig under
`max_standard_scriptsig_size`; the redeem script is `redeem_script_len` bytes.

Activation: `regtest_activation_height` (regtest executes from block 1),
`testnet_activation_height` (see [testnet.md](testnet.md)), and
`mainnet_activation_height`, which is `None`: no mainnet block is ever
interpreted as a Zalkanes message in this release.

There is no gas price, no fee market, and no Zalkanes-level payment: fuel is a
cap, and the only cost is the Zcash transaction fee.

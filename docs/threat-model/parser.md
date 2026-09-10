# Threat Model: Parser

## Attacker goal

Cause a Zalkanes node to panic, allocate unbounded memory, derive incorrect
state, or diverge from other honest nodes by crafting a malicious OP_RETURN
payload or carrier transaction.

## Attack surface

- OP_RETURN payload bytes in any Zcash transparent transaction.
- scriptSig bytes in carrier input(s).
- Transaction ordering within a block.

## Attacks and mitigations

### Panic on malformed input

**Attack:** Supply bytes that trigger a Rust panic (index out of bounds, slice
length mismatch, unwrap on None/Err).

**Mitigation:** Parser uses explicit length checks before every read. All
`Result` paths are handled. Fuzz targets must demonstrate no panic for 10M+
arbitrary inputs before testnet.

### Unbounded allocation

**Attack:** Set `code_length` or `input_length` to u32::MAX, causing a
`Vec::with_capacity` OOM.

**Mitigation:** Every declared length is checked against its protocol maximum
constant BEFORE allocation. e.g.:
```rust
ensure!(declared_len <= MAX_CODE_BYTES, Error::PayloadTooLarge);
let mut buf = vec![0u8; declared_len]; // only after check
```

### Noncanonical / ambiguous encoding

**Attack:** Supply an integer in a non-minimal encoding to produce parser
ambiguity between nodes.

**Mitigation:** Protocol uses fixed-width big-endian integers. No varint.
No encoding that has multiple representations for the same value.

### Trailing bytes

**Attack:** Append extra bytes after a valid message to probe parser behavior
or carry hidden data.

**Mitigation:** Parser MUST consume exactly the declared bytes and REJECT if
any trailing bytes remain. This is tested in the adversarial test suite.

### Wrong magic

**Attack:** Craft an OP_RETURN that starts with bytes that look like ZALK but differ.

**Mitigation:** First 4 bytes are compared to the exact magic constant. Any
mismatch silently skips the output (not an error, not a state change).

### Unknown version / opcode

**Attack:** Use a version byte or message type byte that is not defined in v0
to probe behavior.

**Mitigation:** Unknown version → silently skip. Unknown message type → silently
skip. No state mutation. No panic.

### Duplicate deployment message

**Attack:** Include two DEPLOY messages in the same transaction or block to
attempt duplicate ContractId registration.

**Mitigation:** ContractId is deterministically derived from txid + output_index.
A second message for the same ContractId is silently ignored at the state layer.

### Malformed chunk index

**Attack:** Set chunk_index to a value >= chunk_count, or send duplicate chunk
indexes.

**Mitigation:** Carrier decoder validates that exactly chunk_count distinct
indexes 0..chunk_count-1 are present. Any deviation → reject deployment, no
state mutation.

### Chunk count overflow

**Attack:** Set chunk_count = 255 with very large chunks to exhaust memory.

**Mitigation:** chunk_count is u8 (max 255). Maximum payload is bounded by
code_length <= MAX_CODE_BYTES. Memory allocation is gated on MAX_CODE_BYTES,
not on chunk_count.

### Hash preimage collision attack

**Attack:** Deploy WASM X, then craft a different payload with the same
SHA-256 hash to override it.

**Mitigation:** SHA-256 collision resistance. ContractId also commits to
code_hash AND txid AND output_index, making reuse of a hash in a different
deployment context produce a different ContractId.

## Test coverage

All scenarios above are covered by the parser adversarial test suite (spec §34)
and the fuzz target `fuzz/protocol-frame/` and `fuzz/carrier-decoder/`.

# ADR 0001 — Base Chain Source

**Status:** Accepted  
**Date:** 2025-09-10

## Context

Zalkanes needs a source of validated Zcash block data. Options considered:

1. Embed Zebra consensus logic directly into Zalkanes.
2. Connect to public Zebra RPC endpoints.
3. Connect to a locally operated Zebra instance over authenticated loopback RPC.
4. Use lightwalletd / compact block server.
5. Use a centralized block explorer API.

## Decision

Use option 3: **local Zebra instance over authenticated loopback JSON-RPC**.

The `ChainSource` trait abstracts the connection. The first and only production implementation is `ZebraRpcChainSource`, configured to point to `http://127.0.0.1:8232` by default.

## Rationale

- Zebra is the reference Zcash consensus node. Running it locally gives full block validation without maintaining a consensus fork.
- Loopback-only removes network trust. A remote RPC quorum would require trusting multiple operators.
- lightwalletd and compact-block servers do not provide full transparent transaction data required for carrier decoding.
- Centralized APIs are explicitly out of scope per the security model.

## Consequences

- Production operators must run a local Zebra instance. This is documented in the operator runbook.
- The `ChainSource` trait allows future implementations (e.g., direct Zebra library integration) without changing indexer logic.
- Startup must reject wrong network, unsupported Zebra version, unsynced node in strict mode, and protocol-config hash mismatch.

## Startup validation

```rust
pub fn validate_zebra_connection(client: &ZebraRpcClient) -> Result<()> {
    let info = client.get_blockchain_info()?;
    ensure!(info.chain == expected_network(), "wrong network");
    ensure!(version_compatible(&info.version), "incompatible Zebra version");
    // additional checks in strict mode
    Ok(())
}
```

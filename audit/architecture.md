# Architecture

Canonical: `docs/architecture.md`.

Summary: a local consensus-validating Zebra node is the trust anchor; the
Zalkanes indexer deserializes Zcash blocks via librustzcash, extracts OP_RETURN
messages and carrier chunks, executes WASM via Wasmi against a persistent
RocksDB state, and serves JSON-RPC. See `docs/architecture.md` for the full
component diagram and data flow.

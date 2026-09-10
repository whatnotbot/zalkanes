# ADR 0005 — WASM Consensus Profile

**Status:** Accepted  
**Date:** 2025-09-10

## Context

WASM execution must be deterministic across all nodes on all supported
architectures. The runtime version, feature set, and fuel schedule are
consensus-critical.

## Decision

Use **Wasmi 2.0.0** pinned at `=2.0.0` in `Cargo.toml`.

See `docs/wasm-consensus.md` for the complete execution profile.

Key decisions:

1. **Floats forbidden.** `f32`/`f64` instructions are platform-specific in
   edge cases and are rejected at module validation time.
2. **Threads forbidden.** Non-deterministic by definition.
3. **SIMD forbidden.** Platform-specific semantics.
4. **Fuel is protocol-defined.** The Wasmi 2.0.0 fuel table is frozen.
   Upgrading Wasmi requires a new protocol version + re-vectoring.
5. **Start functions rejected.** Implicit execution at module instantiation
   is not allowed.
6. **Unknown imports rejected.** A module may only import host functions
   listed in the v0 host ABI.

## Rationale

- Wasmi is pure Rust, no-std-capable, and designed for deterministic embedding.
- Version 2.0.0 is the first stable release with the metered fuel API.
- Forbidding floats eliminates the largest source of cross-platform nondeterminism.
- Exact version pin prevents silent Cargo upgrades from changing consensus.

## Stop condition

If fuel consumption differs between x86_64-unknown-linux-gnu and
aarch64-unknown-linux-gnu for any test vector, this ADR must be updated with
an alternative metering scheme before testnet.

## Consequences

- `wasmi = "=2.0.0"` is pinned in workspace Cargo.toml. No `^` or `~`.
- `Cargo.lock` is committed.
- `test-vectors/wasm/fuel-v0.json` vectors are immutable after v0 activation.
- CI fails if code changes a fuel vector output without bumping the protocol version.
- Any Wasmi upgrade opens a protocol upgrade ADR automatically.

# Dependency Lock

Canonical: `docs/upstream-lock.md` (exact versions, commits, and
consensus-critical status).

Pinned and consensus-critical:

| Dependency | Version |
|------------|---------|
| Zebra (trust anchor) | 6.3.0 |
| zcash_primitives | 0.30.1 |
| zcash_protocol | 0.10.6 |
| zcash_transparent | 0.10.0 |
| zcash_script | 0.4.3 |
| wasmi | =2.0.0 |
| Rust toolchain | 1.88.0 |

`Cargo.lock` is committed. See `docs/upstream-lock.md` for the full table and
the V4/V5 sighash branch-id rule.

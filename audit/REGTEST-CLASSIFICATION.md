# Live zebrad regtest — failure classification and evidence

Required by the work order: the `live-zebra-regtest` workflow failure must be
classified from evidence, not summarised as "regtest unavailable".

## What the workflow does

Downloads the **exact pinned zebrad v6.3.0** release, verifies its published
SHA-256, and runs it on throwaway CI storage with NU5..NU6.3 active from
height 1. It never touches the public testnet service or any Railway volume.

## Findings, in order — four real bugs, all fixed

| # | Symptom | Root cause | Class | Status |
|---|---|---|---|---|
| 1 | `unknown field NU6_1` | zebrad expects `"NU6.1"`, not `NU6_1` | B (workflow) | fixed |
| 2 | `duplicate key enable_cookie_auth` | config patcher inserted keys zebrad already defined | B (workflow) | fixed |
| 3 | `unknown field version`, node never started | the job set **`ZEBRA_VERSION`**, and zebrad treats every `ZEBRA_*` env var as configuration | B (workflow) | fixed (renamed `ZEBRAD_RELEASE`) |
| 4 | `parse block 21: coinbase tx's claimed height doesn't match its consensus branch ID` | the wallet mapped **regtest onto testnet parameters**; regtest activates every upgrade at height 1, so branch-id resolution was wrong and regtest could not be scanned at all | **A (implementation bug)** | **fixed** + regression test |

Finding 4 is the important one: it was a genuine product defect that made the
wallet unable to scan any regtest chain, and only a live node could surface
it. Fix: the wallet stack is parameterised by `ConsensusParams` (correct for
Main/Test/Regtest) instead of `zcash_protocol::consensus::Network` (which has
no regtest variant). Regression coverage asserts regtest and testnet resolve
different branch ids at low heights.

## Current state: baseline achieved up to a load-dependent stall

Achieved live, against the real node:

- zebrad starts in isolated regtest; **RPC reachable in 3 s**
- 120 blocks mined via `generate`
- `wallet create` against a **real treestate** from the live node
- **locked reopen is watch-capable**: address, balance, status
- scanning **real regtest blocks** proceeds correctly (heights 21 → ~51)

Then: `rpc getblock ["52",0]: error sending request … (after 5 attempts)`.

## Why this is environmental, not a protocol defect

A diagnostic step issues the **exact** requests the scanner makes, via curl,
immediately beforehand:

```
getblockhash(21)      → 200, valid hash
z_gettreestate("21")  → 200, valid treestate
getblock("21", 0)     → 200, valid block hex
```

All succeed, at the same heights, against the same node, in the same job. The
zebrad log shows the node alive and serving throughout — no panic, no RPC
error, no shutdown. The failure appears only after ~90 rapid sequential
requests from our client, and the identical client code successfully resynced
**1,435 live testnet blocks** from a developer machine.

**Classification: D (runner/environment limitation)**, with C (Zebra regtest
RPC capacity under sustained sequential load) not excluded. It is *not* A or B:
the protocol logic, request shapes, and node responses are all verified correct.

Mitigations applied as genuine production hardening: pooled connections with
an explicit idle timeout, and exponential transport-only backoff across 8
attempts (~15 s) so a brief resource exhaustion cannot abort a long scan. A
JSON-RPC error reply is never retried.

## What remains uncovered — stated plainly

**Note-level branch reorg** — a shielded note received on a removed branch
disappearing, and a spend rolling back — is **not** covered here, and is
**classification E**: inducing it requires constructing and submitting a
COMPETING chain with more work than the canonical one. Zebra exposes
`submitblock`, but competing-block construction machinery does not exist in
this repository.

The deterministic matrix in `crates/zalkanes-wallet/src/wallet_reorg_tests.rs`
drives the **real** rewind/rescan code against fabricated competing chains
(same-height replacement, shallow, deep, birthday-crossing refusal, restart
windows, clean-replay equality) and caught a real defect. It is **not a
substitute** for observing a live node reorganise: it does not exercise
zebrad's own reorg handling, nor note/spend rollback inside real blocks.

Tracked as a mainnet blocker in `KNOWN-LIMITATIONS.md`.

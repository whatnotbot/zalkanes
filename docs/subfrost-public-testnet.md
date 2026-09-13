# SUBFROST AMM v0 — public Zcash Testnet procedure

## Current status: BLOCKED-UPSTREAM (no funding needed, no wallet made)

The DEX contracts require the proposed host-ABI extension
(`docs/subfrost-amm-v0.md`, "Upstream blockers" UB-1..UB-6). The frozen
v0 platform's `validate_module` rejects any module importing beyond the
six-function allowlist, so the pool/factory contracts **cannot deploy**
on regtest or public testnet today. Consequently:

- AC-TNET-04..10 = **BLOCKED-UPSTREAM**.
- No testnet wallet was generated and **no TAZ is requested** — funds
  are only requested when a deployment path actually exists (spec §43).
- Deployment semantics were **not** invented and consensus was **not**
  modified to force this row green (spec §46, §65).

What *is* possible on the frozen platform today — and is already
validated through the real `zalkanes-testkit` (real parser, carriers,
consensus wasmi, state roots): deploying `contracts/subfrost-mathcheck`
and executing the frozen AMM arithmetic on-chain. That path would also
work on public testnet via the existing `zalkanes contract deploy/call`
CLI, but it is a math probe, not the DEX, so it does not satisfy
AC-TNET-05..08 and is not worth spending faucet TAZ on.

## Procedure once the host ABI lands upstream

Preconditions (all already true at the application layer):
math tests green; contract tests green; deterministic replay green;
wallet/scanner path healthy; dry-run structural validation green.

1. **Wallet**: create a NEW dedicated TESTNET-ONLY wallet with
   `zalkanes wallet create` (canonical implementation; encrypted local
   custody). Never reuse a mainnet seed; never print/commit/log the
   seed; never send the secret anywhere. Prefer a Unified Address;
   derive the transparent receiver only if the deploy path needs it.
2. **Funding calculation** (spec §44): count transactions —
   3 deployments (token A, token B, factory) × (PREPARE + DEPLOY)
   + 1 create-pool CALL + 1 swap CALL + carrier dust + ZIP-317 fees via
   the actual `zalkanes-tx` builder; request
   `max(exact_need * 3, faucet_minimum)` and print the standard
   FUNDING REQUIRED packet. Stop only for the faucet transfer.
3. **Funding verification** (spec §45): trust only own Zebra → canonical
   block → wallet full-block scanner → WalletDb; require expected value,
   pool, confirmations, and wallet tip == Zebra tip.
4. **Smoke deployment** (spec §47): deploy test token A, test token B,
   factory; create ONE pool; execute ONE swap; record txids, heights,
   block hashes, contract ids, code hashes, reserves before/after,
   quote vs actual, LP supply, protocol fees, state roots. Restart all
   local services and verify identical state.
5. **Second-node verification** (spec §48): run Zebra A→Zalkanes A and
   Zebra B→Zalkanes B against public testnet; require `root_A == root_B`
   at the deployment/swap heights. Document if only one implementation
   is available.

## Mainnet: never

`MAINNET_ACTIVATION_HEIGHT` is `None` and stays `None`. The platform CLI
refuses mainnet without `--confirm-mainnet`; the DEX CLI refuses mainnet
DEX operations **unconditionally** (tested in
`crates/zalkanes-dex-cli/src/main.rs`). No real ZEC, no mainnet canary,
no "tiny amount".

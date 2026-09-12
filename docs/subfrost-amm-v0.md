# SUBFROST AMM v0 on Zalkanes

> **This implementation ports the AMM architecture of SUBFROST/Alkanes to
> Zalkanes. It does not implement SUBFROST's FROST Bitcoin-custody
> subsystem.**

Branch: `feat/subfrost-amm-v0`. Status: application layer complete and
validated; on-chain deployment **BLOCKED-UPSTREAM** (see "Upstream
blockers" — the frozen v0 host ABI cannot execute these contracts yet).

## Scope

In scope (v0): constant-product AMM — permissionless factory, pools,
LP tokens, add/remove liquidity, direct exact-input swaps, quotes,
views, deterministic events, test tokens, SDK intents, dry-run CLI.

Out of scope (explicit non-goals): FROST/ROAST/DKG custody, frBTC/frZEC,
bridges, shielded execution, lending, governance/protocol tokens,
staking, concentrated liquidity, order books, routing/aggregation,
oracles/TWAP (op 98 returns `Unsupported`), MEV systems, frontends,
hosted indexers, production deployment infrastructure.

## Architecture

```
crates/zalkanes-dex-core     pure protocol layer: types, v0 constants,
                             integer math, errors, encodings, host ABI trait
crates/zalkanes-dex-wasm     wasm32 entry glue for the PROPOSED ABI
crates/zalkanes-dex-sdk      intents -> CallPlan builders, views
crates/zalkanes-dex-testkit  deterministic in-memory runtime (Host impl):
                             atomic calls, blocks, rollback, restart,
                             state roots, fuel, fault injection
crates/zalkanes-dex-cli      `zalkanes-dex` binary (dry-run only)
contracts/test-token         deterministic test-only token
contracts/subfrost-pool      the AMM pool (LP identity == pool id)
contracts/subfrost-factory   permissionless factory + registry
contracts/subfrost-mathcheck AMM math under the REAL frozen six-import
                             ABI; deployable + validated on consensus
                             wasmi via the real zalkanes-testkit
```

Upstream reference: see `docs/subfrost-upstream-parity.md`
(`Oyl-Wallet/oyl-amm @ 80b04f3d`, `kungfuflex/alkanes-rs @ 62511e93`),
including all twelve intentional deviations.

## Frozen v0 constants (`zalkanes-dex-core/src/v0.rs`)

```
FEE_DENOMINATOR_BPS = 10_000
TOTAL_SWAP_FEE_BPS  = 100     (1.00%)
LP_FEE_BPS          = 80      (0.80%)
PROTOCOL_FEE_BPS    = 20      (0.20%)
MINIMUM_LIQUIDITY   = 1_000
PAIR_KEY_DOMAIN     = "zalkanes-subfrost-pair-v0"
```

Changing any value is a new protocol version and requires regenerating
`test-vectors/subfrost_amm_v0_vectors.json`.

## Exact arithmetic (all floor, all integer, no floats)

Initial liquidity (256-bit product, binary-search sqrt):

```
gross_lp    = floor(sqrt(amount0 * amount1))
require       gross_lp > MINIMUM_LIQUIDITY   else InsufficientInitialLiquidity
provider_lp = gross_lp - MINIMUM_LIQUIDITY
total_lp    = gross_lp        # the locked 1000 is counted, held by nobody
```

Add liquidity (upstream branch rule):

```
optimal1 = floor(desired0 * reserve1 / reserve0)
if optimal1 <= desired1: accept (desired0, optimal1)
else:                    accept (floor(desired1 * reserve0 / reserve1), desired1)
lp_minted = min(floor(a0*S/R0), floor(a1*S/R1));  0 -> InsufficientLiquidity
refunds   = desired - accepted (returned in the same call)
```

Remove liquidity:

```
amount_i = floor(lp * reserve_i / S)
either amount == 0 -> InsufficientLiquidity   (upstream parity)
lp > S -> InsufficientLp
```

Swap (exact input; upstream denominator-first `get_amount_out`,
generalized per-1000 -> bps; floor-for-floor identical at 1.00%):

```
require amount_in >= 100 (total_fee > 0)   else ZeroAmount   [v0 deviation]
wf         = amount_in * (10_000 - 100)          # overflow -> error
amount_out = floor(wf * reserve_out / (reserve_in * 10_000 + wf))
require amount_out > 0 else InsufficientLiquidity; amount_out < reserve_out always

total_fee    = floor(amount_in * 100 / 10_000)
lp_fee       = floor(amount_in *  80 / 10_000)
protocol_fee = total_fee - lp_fee
```

Reserve/fee accounting per swap:

```
reserve_in  += amount_in - protocol_fee    # LP share stays in reserves
reserve_out -= amount_out
protocol_fees_in += protocol_fee           # accumulator, excluded from pricing
```

Protocol-fee custody model: **model A** — protocol fees remain
physically inside pool custody but are excluded from LP reserves and
tracked in `protocol_fees0/1`. v0 has **no collection operation, no
admin keys, no treasury**. Invariant (tested): pool custody of token i
`== reserve_i + protocol_fees_i` (+ inert donations, below).

Wide arithmetic: 256-bit `mul_wide`/`mul_div_floor`/`sqrt_wide` in
`zalkanes-dex-core/src/wide.rs`; every narrowing checked; every error
deterministically reverts.

## ABI

### Pool (`contracts/subfrost-pool`)

| opcode | op | calldata | attached assets |
|---|---|---|---|
| 0 | initialize (factory only, once) | token0(32) token1(32) provider(33) | both tokens |
| 1 | add_liquidity | min_lp_out(16) expiry(4) | both tokens = desired amounts |
| 2 | remove_liquidity | min0(16) min1(16) expiry(4) | LP only |
| 3 | swap_exact_in | min_out(16) expiry(4) | exactly one pool token |
| 97 | get_reserves | — | — |
| 98 | cumulative price | — | returns `Unsupported` (21) |
| 99 | get_name ("A / B LP") | — | — |
| 100 | quote_exact_in | token_in(32) amount_in(16) | — |
| 999 | pool_details | — | — |

All integers big-endian (platform convention; upstream uses LE —
deviation 7). Results: reserves = r0‖r1 (16+16); details = pool‖factory‖
token0‖token1‖r0‖r1‖supply‖pfees0‖pfees1 (208 bytes); quote = token_in‖
token_out‖amount_in‖amount_out‖total‖lp‖protocol (144 bytes).

Expiry rule: `0` disables; otherwise `height > expiry -> Expired`.
Height always comes from the execution context, never the caller.

### Factory (`contracts/subfrost-factory`)

| opcode | op |
|---|---|
| 0 | initialize(pool_template_hash 32) — once; fixes the template |
| 1 | create_pool(tokenA 32 ‖ tokenB 32) + attached initial amounts |
| 2 | get_pool(tokenA ‖ tokenB) -> pool_id or empty |
| 3 | get_all_pools([start u32 ‖ limit u32]) -> count ‖ ids, creation order |
| 4 | pool_count() -> u128 |

Permissionless; no owner, no allowlist, no admin opcode (tested:
FACTORY-011/012). Flow: canonicalize pair → reject identical/duplicate →
validate attached amounts → spawn pool → initialize pool (forwarding
assets, provider = caller) → register **only after** success → emit
`PoolCreated`. Entirely atomic.

### Test token (`contracts/test-token`)

`0 initialize(name)`, `1 mint_for_test(to 33 ‖ amount 16)`
(deliberately permissionless — test-only), `99 get_name`.
Fixtures: "Zalkanes Test Asset A/B/C". Never named ZEC/frZEC/wrapped ZEC.

## Identity, pair ordering, storage

- `ContractId`: reused verbatim from `zalkanes-core` (no parallel type).
- `AssetId` (new, part of the ABI proposal): 32 bytes; a contract-native
  asset's id == the contract's id bytes. **LP AssetId == pool ContractId.**
- Canonical pair: `token0 = min(a, b)`, `token1 = max(a, b)` by byte
  order; identical assets rejected; `canonical(A,B) == canonical(B,A)`
  (PROP-001, fuzzed).
- Pair key: `"zalkanes-subfrost-pair-v0" ‖ token0 ‖ token1` — fixed-width
  domain-separated concatenation; collision-free without new crypto.
- Pool storage keys: `init factory token0 token1 reserve0 reserve1
  lp_supply pfees0 pfees1 lock name`.
- Factory storage keys: `init template count pair/<pairkey>
  pool/<index_be32> poolpair/<pool_id>`.
- Determinism: no HashMap iteration, no time, no randomness, no I/O in
  any consensus-relevant path; enumeration preserves creation order
  (FACTORY-007/010).

## Errors (stable codes)

`AlreadyInitialized=1 NotInitialized=2 UnauthorizedInitialize=3
IdenticalAssets=4 DuplicatePool=5 InvalidAsset=6 InvalidIncomingAssets=7
ZeroAmount=8 InsufficientInitialLiquidity=9 InsufficientLiquidity=10
InsufficientLp=11 SlippageExceeded=12 Expired=13 ArithmeticOverflow=14
ArithmeticUnderflow=15 DivisionByZero=16 InvalidOpcode=17
InvalidArguments=18 Reentrancy=19 OutOfFuel=20 Unsupported=21`

Every code is exercised by tests. Codes never renumber; only append.

## Events (§24)

Tag ‖ fixed-width BE fields; emitted only for calls that commit — a
reverting call leaves no event (tested). `PoolCreated(0x01)`
`LiquidityAdded(0x02)` `LiquidityRemoved(0x03)` `Swap(0x04)` with
exactly the spec fields. `lp_minted` in `PoolCreated` = provider LP
(excludes the locked minimum).

## Reentrancy

Storage `lock` guard wraps every mutating pool op (upstream `/lock`
parity). `initialize` writes complete state **and takes the lock before
any outbound call** (token-name resolution), so a malicious token
calling back in hits `AlreadyInitialized`/`Reentrancy`, never a
half-initialized pool — adversarially tested with reentering, trapping,
garbage-name and fuel-burning tokens. In the v0 model asset transfers
are passive ledger mutations (no code runs on transfer), so no organic
reentrancy vector exists; the guard is enforced defense-in-depth and is
additionally tested via fault injection.

## Reserve-sync invariant (§59)

`reserve0/1` are **accounting state**, not live custody. Unsolicited
transfers (donations) into pool custody do not change reserves, quotes
or LP redemptions (tested); the surplus is permanently inert. v0 has no
sync/skim.

## Determinism & rollback

- The DEX testkit models blocks with per-height snapshots; rollback uses
  snapshot restore (no DEX-specific database; the real platform's
  RocksDB rollback machinery remains the production answer).
- Deterministic replay: the full E2E sequence on two pristine chains
  produces identical roots at **every** height, identical events,
  identical fuel (tested).
- Reorg: branch A → rollback → branch B equals a clean branch-B reindex
  root; restart (serialize/restore) preserves the root (tested).
- Real-platform bridge: `contracts/subfrost-mathcheck` (six-import ABI,
  strict no_std) passes the **real consensus WASM validator** and
  reproduces the AMM arithmetic bit-identically under the **real wasmi
  engine** with deterministic per-block state roots
  (`crates/zalkanes-dex-testkit/tests/real_platform.rs`).

## Fuel

DEX-testkit fuel schedule (deterministic): explicit per-op base charges
in the contracts + per-host-op charges in the runtime. Regression
ceilings enforced by `tests/fuel.rs`; swap fuel and pool lookup fuel are
proven independent of registry size (no unbounded loops; enumeration is
the only O(n) op and supports pagination). On the real platform,
mathcheck executes under genuine wasmi instruction metering.

## Testing summary

- 41 pure math tests (MATH-001..054), 10 property suites (PROP-001..020,
  fixed-seed, thousands of cases), 53 golden vectors generated by an
  independent Python big-integer reference.
- 79 runtime tests: POOL-001..035, FACTORY-001..012, atomicity matrix,
  E2E/replay/reorg/restart, security (malicious tokens, donations,
  fuzz-style dispatch robustness), fuel, SDK.
- 6 real-platform tests on consensus wasmi; 4 CLI tests.
- 9 cargo-fuzz targets (`fuzz/fuzz_targets/dex_*`), all compiled and
  smoke-run crash-free.

## Reproducible WASM hashes (SOURCE_DATE_EPOCH=0, RUSTFLAGS="-C link-arg=-s")

Two isolated clean builds are byte-identical. SHA-256:

```
7a97d5ce1279ac2ff0cb3bb14edbf8cba00bf06cfe5a8d541de004bcd6dc6254  subfrost_factory.wasm
8492b69a5f633531b32103c1f835d9472c285d5a9ae8f1bd9faac3d1f2f0711c  subfrost_mathcheck.wasm
19c946f76564b8ff3f4bb1b082e199faff6023e6da186263422476c49aa1f666  subfrost_pool.wasm
a579ad102669ebd7535a8cf5e80835bf2d6d9b68e5b27bbb9034ef58736b08ba  test_token.wasm
```

Committed fixtures in `crates/zalkanes-dex-testkit/fixtures/` carry
these exact bytes (the mathcheck fixture is the deployable one).

## Measured fuel (DEX-testkit schedule, deterministic)

```
create_pool 7951   swap 1453   add 1814   remove 1874
quote 300          details 300
Ceilings: create 20000, swap/add/remove 4000, views 2000 (tests/fuel.rs)
```

## Upstream blockers (BLOCKED-UPSTREAM)

The frozen Zalkanes v0 host ABI exposes exactly six imports
(`storage_get/set/delete`, `context_block_height`, `input_read`,
`output_write`). The DEX additionally requires (full proposal:
`zalkanes-dex-core/src/host.rs`; wire sketch:
`crates/zalkanes-dex-wasm`):

- **UB-1 assets**: native `AssetId` ledger with mint/burn/transfer and
  call-attached value (`dex_incoming_*`, `dex_transfer_out`,
  `dex_mint_own`, `dex_burn_own`).
- **UB-2 caller identity**: `dex_caller` (`context_caller` is documented
  in `docs/wasm-consensus.md` §6 but not implemented; minimal
  reproducer: `crates/zalkanes-runtime/tests/adversarial_wasm.rs:139`
  proves `env::contract_call`-style imports are rejected).
- **UB-3 cross-contract calls**: `dex_call` with revert semantics
  (`audit/README.md`: "There is no cross-contract call in v0").
- **UB-4 contract spawn**: `dex_spawn` (pools are spawned by the
  factory; v0 contracts exist only via DEPLOY transactions).
- **UB-5 sequential intra-block visibility**: v0 executes every call in
  a block against pre-block state, last write wins
  (`crates/zalkanes-indexer/src/lib.rs:511`, locked in by
  `crates/zalkanes-testkit/tests/execution_vectors.rs:177`). Two swaps
  in one Zcash block would silently clobber each other — disqualifying
  for an AMM. The DEX requires sequential visible execution.
- **UB-6 events**: a `dex_emit_event` facility (only `return_data`
  exists today).

Per AGENTS.md every one of these is a host-ABI/consensus change
requiring an ADR + protocol version bump + new test vectors — exactly
the boundary this branch does **not** cross. Nothing in the frozen
platform was modified beyond one additive `Ord` derive on `ContractId`
(byte-lexicographic, no consensus meaning) and workspace/member wiring.

## Public testnet procedure

See `docs/subfrost-public-testnet.md`. Current status:
**BLOCKED-UPSTREAM** (no deployable DEX transport exists, so no wallet
was generated and no TAZ is requested). `MAINNET_ACTIVATION_HEIGHT`
remains `None`; the CLI refuses mainnet DEX operations outright
(tested).

## Security invariants (enforced by tests)

1. No successful swap drains a reserve (`amount_out < reserve_out`).
2. `lp_fee + protocol_fee == total_fee` exactly, per swap.
3. Asset conservation: caller + pool + protocol accounting is constant
   across every operation (property + fuzz + runtime tests).
4. LP conservation: circulating LP + locked minimum == supply.
5. The locked minimum is unredeemable (POOL-008, MATH-033).
6. Any failure — slippage, expiry, overflow, fuel, malformed calldata,
   reentrancy, wrong assets — reverts atomically (state root proof).
7. Registration only after successful pool initialization.
8. No admin capability exists anywhere (FACTORY-012).
9. Donations cannot move the price (§59).
10. Malicious tokens (reentering / trapping / garbage / fuel-burning)
    cannot corrupt pool state (§60).

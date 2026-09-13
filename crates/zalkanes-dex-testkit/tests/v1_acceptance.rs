//! PROTOCOL V1 acceptance (ADR-0008 / Track B): the UNCHANGED SUBFROST
//! pool/factory/test-token WASM running on the REAL platform — real
//! protocol parser (V1 CALL commitments + carrier payloads + secp256k1
//! auth), real consensus wasmi via the V1 engine, real state roots with
//! the asset ledger, through `zalkanes-testkit` blocks.
//!
//! Acceptance items covered here:
//!   - two swaps in the same block execute sequentially + deterministically
//!   - a failed second swap does not corrupt the first
//!   - cross-contract revert is atomic (factory spawn+init)
//!   - asset conservation exact
//!   - caller identity exact
//!   - pool spawn atomic
//!   - events deterministic
//!   - rollback/reorg exact; clean reindex equal
//!   - two independent nodes equal roots per block
//!   - malformed input deterministic
//! (Restart-equal for the ledger is covered at the state layer:
//! `zalkanes-state/tests/ledger_v1.rs`.)

use zalkanes_core::types::{CodeHash, ContractId, Execution, Network};
use zalkanes_dex_core::encode::{
    decode_pool_details, decode_reserves, factory_create_pool_args, factory_initialize_args,
    factory_op, pool_op, pool_swap_args, token_initialize_args, token_mint_args, token_op,
};
use zalkanes_dex_core::math::quote_swap_exact_in;
use zalkanes_dex_core::types::Holder;
use zalkanes_state::StateStore;
use zalkanes_testkit::{contract_holder, external_holder_for, test_signer, TestChain, V1Call};

const TOKEN_WASM: &[u8] = include_bytes!("../fixtures/test_token.wasm");
const POOL_WASM: &[u8] = include_bytes!("../fixtures/subfrost_pool.wasm");
const FACTORY_WASM: &[u8] = include_bytes!("../fixtures/subfrost_factory.wasm");

const INIT_A: u128 = 5_000_000;
const INIT_B: u128 = 20_000_000;
const ALICE_FUNDS: u128 = 1_000_000_000;
const BOB_FUNDS: u128 = 50_000_000;

struct Dex {
    chain: TestChain,
    factory: ContractId,
    pool: ContractId,
    za: [u8; 32],
    zb: [u8; 32],
    alice: secp256k1::SecretKey,
    bob: secp256k1::SecretKey,
    alice_holder: [u8; 33],
    bob_holder: [u8; 33],
}

fn dex_holder(raw: &[u8; 33]) -> Holder {
    Holder::from_bytes(raw).expect("valid holder")
}

/// Deterministic full setup on a pristine chain: tokens, factory,
/// funded accounts, and the ZA/ZB pool created through the factory
/// (spawn + init on the real V1 engine).
fn setup() -> Dex {
    let mut chain = TestChain::new();
    chain.mine_empty_block().expect("pre-V1 block");

    let token_a = chain.deploy_v1(TOKEN_WASM).expect("deploy token A");
    let token_b = chain.deploy_v1(TOKEN_WASM).expect("deploy token B");
    let factory = chain.deploy_v1(FACTORY_WASM).expect("deploy factory");
    // The pool template: deployed once so its code is on-chain for spawn.
    let _pool_template = chain.deploy_v1(POOL_WASM).expect("deploy pool template");
    let (za, zb) = (token_a.0, token_b.0);

    let alice = test_signer(1);
    let bob = test_signer(2);
    let alice_holder = external_holder_for(Network::Regtest, &alice);
    let bob_holder = external_holder_for(Network::Regtest, &bob);

    for (token, name) in [
        (token_a, "Zalkanes Test Asset A"),
        (token_b, "Zalkanes Test Asset B"),
    ] {
        let record = chain
            .call_v1(V1Call::new(
                token,
                token_op::INITIALIZE,
                token_initialize_args(name),
            ))
            .expect("token init");
        assert!(record.success, "token init failed: {:?}", record.error);
    }
    for (token, to, amount) in [
        (token_a, alice_holder, ALICE_FUNDS),
        (token_b, alice_holder, ALICE_FUNDS),
        (token_a, bob_holder, BOB_FUNDS),
    ] {
        let record = chain
            .call_v1(V1Call::new(
                token,
                token_op::MINT_FOR_TEST,
                token_mint_args(&dex_holder(&to), amount),
            ))
            .expect("mint");
        assert!(record.success, "mint failed: {:?}", record.error);
    }

    let record = chain
        .call_v1(V1Call::new(
            factory,
            factory_op::INITIALIZE,
            factory_initialize_args(&CodeHash::of(POOL_WASM).0),
        ))
        .expect("factory init");
    assert!(record.success, "factory init failed: {:?}", record.error);

    // Create the pool: attach both initial amounts, signed by alice.
    let mut create = V1Call::new(
        factory,
        factory_op::CREATE_POOL,
        factory_create_pool_args(
            &zalkanes_dex_core::types::AssetId(za),
            &zalkanes_dex_core::types::AssetId(zb),
        ),
    );
    create.attached = vec![(za, INIT_A), (zb, INIT_B)];
    create.signer = Some(alice);
    let record = chain.call_v1(create).expect("create pool");
    assert!(record.success, "create pool failed: {:?}", record.error);
    assert_eq!(record.return_data.len(), 32, "pool id returned");
    let mut pool_id = [0u8; 32];
    pool_id.copy_from_slice(&record.return_data);
    let pool = ContractId(pool_id);

    Dex {
        chain,
        factory,
        pool,
        za,
        zb,
        alice,
        bob,
        alice_holder,
        bob_holder,
    }
}

fn reserves(dex: &Dex) -> (u128, u128) {
    decode_reserves(
        &dex.chain
            .view_v1(dex.pool, pool_op::GET_RESERVES, &[])
            .expect("reserves view"),
    )
    .expect("decode reserves")
}

/// Map (reserve0, reserve1) onto (ZA-reserve, ZB-reserve).
fn oriented_reserves(dex: &Dex) -> (u128, u128) {
    let (r0, r1) = reserves(dex);
    if dex.za < dex.zb {
        (r0, r1)
    } else {
        (r1, r0)
    }
}

fn swap_call(dex: &Dex, signer: secp256k1::SecretKey, asset: [u8; 32], amount: u128) -> V1Call {
    let mut call = V1Call::new(dex.pool, pool_op::SWAP_EXACT_IN, pool_swap_args(0, 0));
    call.attached = vec![(asset, amount)];
    call.signer = Some(signer);
    call
}

#[test]
fn setup_spawns_pool_with_exact_lp_and_reserves() {
    let dex = setup();
    // Pool exists as a spawned contract with the template's code hash.
    let (code_hash, _) = dex
        .chain
        .state()
        .get_contract(&dex.pool)
        .expect("spawned pool persisted as a contract");
    assert_eq!(code_hash, CodeHash::of(POOL_WASM));

    let (ra, rb) = oriented_reserves(&dex);
    assert_eq!((ra, rb), (INIT_A, INIT_B));

    // isqrt(5e6 * 2e7) = 1e7; provider LP = 1e7 - 1000, owned by alice's
    // EXACT ExternalId (caller identity), as the pool's own asset id.
    let lp_asset = dex.pool.0;
    assert_eq!(
        dex.chain.ledger_balance(&dex.alice_holder, &lp_asset),
        10_000_000 - 1_000
    );
    // Custody: pool holds both tokens.
    let pool_holder = contract_holder(&dex.pool);
    assert_eq!(dex.chain.ledger_balance(&pool_holder, &dex.za), INIT_A);
    assert_eq!(dex.chain.ledger_balance(&pool_holder, &dex.zb), INIT_B);
}

#[test]
fn two_swaps_same_block_execute_sequentially_and_deterministically() {
    let run = || {
        let mut dex = setup();
        let (ra, rb) = oriented_reserves(&dex);
        // Expected sequential outcomes from the frozen v0 math:
        let q1 = quote_swap_exact_in(ra, rb, 1_000_000).expect("q1");
        let ra2 = ra + 1_000_000 - q1.fees.protocol_fee;
        let rb2 = rb - q1.amount_out;
        let q2 = quote_swap_exact_in(ra2, rb2, 1_000_000).expect("q2");
        assert_ne!(
            q1.amount_out, q2.amount_out,
            "sequential pricing must differ from pre-block pricing"
        );

        let records = dex
            .chain
            .calls_v1_block(vec![
                swap_call(&dex, dex.alice, dex.za, 1_000_000),
                swap_call(&dex, dex.bob, dex.za, 1_000_000),
            ])
            .expect("two-swap block");
        assert!(records[0].success, "swap1: {:?}", records[0].error);
        assert!(records[1].success, "swap2: {:?}", records[1].error);

        // Message N+1 saw message N's state: outputs match sequential quotes.
        let out1 = u128::from_be_bytes(records[0].return_data[..16].try_into().unwrap());
        let out2 = u128::from_be_bytes(records[1].return_data[..16].try_into().unwrap());
        assert_eq!(out1, q1.amount_out, "first swap = pre-state quote");
        assert_eq!(out2, q2.amount_out, "second swap = post-first-swap quote");

        // Buyers actually received those amounts.
        assert_eq!(
            dex.chain.ledger_balance(&dex.alice_holder, &dex.zb),
            ALICE_FUNDS - INIT_B + q1.amount_out
        );
        assert_eq!(
            dex.chain.ledger_balance(&dex.bob_holder, &dex.zb),
            q2.amount_out
        );
        (dex.chain.state().compute_root(), records)
    };
    let (root1, records1) = run();
    let (root2, records2) = run();
    assert_eq!(root1, root2, "two-swap block deterministic across nodes");
    assert_eq!(records1, records2, "identical execution records");
}

#[test]
fn failed_second_swap_does_not_corrupt_the_first() {
    let mut dex = setup();
    let (ra, rb) = oriented_reserves(&dex);
    let q1 = quote_swap_exact_in(ra, rb, 1_000_000).expect("q1");

    // Second swap demands an impossible min_out -> SlippageExceeded (12).
    let mut failing = V1Call::new(
        dex.pool,
        pool_op::SWAP_EXACT_IN,
        pool_swap_args(u128::MAX, 0),
    );
    failing.attached = vec![(dex.za, 1_000_000)];
    failing.signer = Some(dex.bob);

    let records = dex
        .chain
        .calls_v1_block(vec![swap_call(&dex, dex.alice, dex.za, 1_000_000), failing])
        .expect("block");
    assert!(records[0].success);
    assert!(!records[1].success);
    assert!(
        records[1]
            .error
            .as_deref()
            .unwrap_or("")
            .contains("code 12"),
        "expected SlippageExceeded, got {:?}",
        records[1].error
    );

    // First swap fully intact; bob completely untouched.
    let (ra_after, rb_after) = oriented_reserves(&dex);
    assert_eq!(ra_after, ra + 1_000_000 - q1.fees.protocol_fee);
    assert_eq!(rb_after, rb - q1.amount_out);
    assert_eq!(
        dex.chain.ledger_balance(&dex.bob_holder, &dex.za),
        BOB_FUNDS
    );
    assert_eq!(dex.chain.ledger_balance(&dex.bob_holder, &dex.zb), 0);

    // The state root equals a chain where ONLY the first swap ran.
    let mut only_first = setup();
    let records = only_first
        .chain
        .calls_v1_block(vec![swap_call(
            &only_first,
            only_first.alice,
            only_first.za,
            1_000_000,
        )])
        .expect("single-swap block");
    assert!(records[0].success);
    assert_eq!(
        dex.chain.state().compute_root(),
        only_first.chain.state().compute_root(),
        "failed second swap left zero residue in the state root"
    );
}

#[test]
fn cross_contract_revert_is_atomic() {
    let mut dex = setup();
    let root_before = dex.chain.state().compute_root();

    // Duplicate pair -> factory rejects AFTER canonicalization (code 5);
    // nothing changes: attachments refunded, no spawn, no registry entry.
    let mut duplicate = V1Call::new(
        dex.factory,
        factory_op::CREATE_POOL,
        factory_create_pool_args(
            &zalkanes_dex_core::types::AssetId(dex.zb),
            &zalkanes_dex_core::types::AssetId(dex.za),
        ),
    );
    duplicate.attached = vec![(dex.za, INIT_A), (dex.zb, INIT_B)];
    duplicate.signer = Some(dex.alice);
    let record = dex.chain.call_v1(duplicate).expect("block");
    assert!(!record.success);
    assert!(record.error.as_deref().unwrap_or("").contains("code 5"));
    assert_eq!(dex.chain.state().compute_root(), root_before);

    // Insufficient initial liquidity on a NEW pair: the pool spawn +
    // initialize chain reverts atomically inside the factory call.
    let token_c = dex.chain.deploy_v1(TOKEN_WASM).expect("token C");
    let record = dex
        .chain
        .call_v1(V1Call::new(
            token_c,
            token_op::INITIALIZE,
            token_initialize_args("Zalkanes Test Asset C"),
        ))
        .expect("init C");
    assert!(record.success);
    let record = dex
        .chain
        .call_v1(V1Call::new(
            token_c,
            token_op::MINT_FOR_TEST,
            token_mint_args(&dex_holder(&dex.alice_holder), 10_000),
        ))
        .expect("mint C");
    assert!(record.success);
    let root_before = dex.chain.state().compute_root();
    let alice_za = dex.chain.ledger_balance(&dex.alice_holder, &dex.za);

    let mut tiny = V1Call::new(
        dex.factory,
        factory_op::CREATE_POOL,
        factory_create_pool_args(
            &zalkanes_dex_core::types::AssetId(dex.za),
            &zalkanes_dex_core::types::AssetId(token_c.0),
        ),
    );
    tiny.attached = vec![(dex.za, 500), (token_c.0, 500)];
    tiny.signer = Some(dex.alice);
    let record = dex.chain.call_v1(tiny).expect("block");
    assert!(!record.success);
    assert!(
        record.error.as_deref().unwrap_or("").contains("code 9"),
        "expected InsufficientInitialLiquidity, got {:?}",
        record.error
    );
    assert_eq!(
        dex.chain.state().compute_root(),
        root_before,
        "spawn + init revert left zero residue"
    );
    assert_eq!(
        dex.chain.ledger_balance(&dex.alice_holder, &dex.za),
        alice_za,
        "attachments refunded"
    );
}

#[test]
fn asset_conservation_is_exact() {
    let mut dex = setup();
    let total_of = |chain: &TestChain, asset: &[u8; 32]| -> u128 {
        let mut sum = 0u128;
        for ((_holder, a), amount) in &chain.state().ledger {
            if a == asset {
                sum += *amount;
            }
        }
        sum
    };
    let za_total = total_of(&dex.chain, &dex.za);
    let zb_total = total_of(&dex.chain, &dex.zb);
    assert_eq!(za_total, ALICE_FUNDS + BOB_FUNDS);
    assert_eq!(zb_total, ALICE_FUNDS);

    // A mixed block of swaps (one failing) conserves both assets exactly.
    let mut failing = V1Call::new(
        dex.pool,
        pool_op::SWAP_EXACT_IN,
        pool_swap_args(u128::MAX, 0),
    );
    failing.attached = vec![(dex.zb, 2_000_000)];
    failing.signer = Some(dex.alice);
    let records = dex
        .chain
        .calls_v1_block(vec![
            swap_call(&dex, dex.alice, dex.za, 777_777),
            failing,
            swap_call(&dex, dex.bob, dex.za, 123_456),
        ])
        .expect("block");
    assert!(records[0].success && !records[1].success && records[2].success);
    assert_eq!(total_of(&dex.chain, &dex.za), za_total, "ZA conserved");
    assert_eq!(total_of(&dex.chain, &dex.zb), zb_total, "ZB conserved");
}

#[test]
fn caller_identity_is_exact() {
    let mut dex = setup();
    // An EXTERNAL caller cannot initialize a pool (factory-only), even with
    // valid assets attached: UnauthorizedInitialize (code 3).
    let orphan = dex.chain.deploy_v1(POOL_WASM).expect("orphan pool");
    let (t0, t1) = if dex.za < dex.zb {
        (dex.za, dex.zb)
    } else {
        (dex.zb, dex.za)
    };
    let mut init = V1Call::new(
        orphan,
        pool_op::INITIALIZE,
        zalkanes_dex_core::encode::pool_initialize_args(
            &zalkanes_dex_core::types::AssetId(t0),
            &zalkanes_dex_core::types::AssetId(t1),
            &dex_holder(&dex.alice_holder),
        ),
    );
    init.attached = vec![(dex.za, INIT_A), (dex.zb, INIT_B)];
    init.signer = Some(dex.alice);
    let record = dex.chain.call_v1(init).expect("block");
    assert!(!record.success);
    assert!(
        record.error.as_deref().unwrap_or("").contains("code 3"),
        "expected UnauthorizedInitialize, got {:?}",
        record.error
    );

    // Unauthenticated attachments are impossible by construction: the
    // payload decoder rejects them (protocol violation, no execution).
    let mut unsigned = V1Call::new(dex.pool, pool_op::SWAP_EXACT_IN, pool_swap_args(0, 0));
    unsigned.attached = vec![(dex.za, 1_000)];
    let err = dex.chain.call_v1(unsigned).expect_err("no record");
    assert!(err.to_string().contains("missing execution record"));
}

#[test]
fn swap_events_are_deterministic_and_decodable() {
    let run = || {
        let mut dex = setup();
        let records = dex
            .chain
            .calls_v1_block(vec![swap_call(&dex, dex.alice, dex.za, 1_000_000)])
            .expect("block");
        assert!(records[0].success);
        records[0].clone()
    };
    let r1 = run();
    let r2 = run();
    assert_eq!(r1.events, r2.events, "event bytes deterministic");
    assert_eq!(r1.events.len(), 1, "exactly one Swap event");
    let (emitter, bytes) = &r1.events[0];
    let event = zalkanes_dex_core::events::DexEvent::decode(bytes).expect("decodable");
    match event {
        zalkanes_dex_core::events::DexEvent::Swap {
            pool, amount_in, ..
        } => {
            assert_eq!(pool.0, emitter.0);
            assert_eq!(amount_in, 1_000_000);
        }
        other => panic!("expected Swap event, got {other:?}"),
    }
}

#[test]
fn rollback_reorg_and_clean_reindex_are_exact() {
    let mut dex = setup();
    let ancestor_height = {
        let h = dex.chain.height();
        h
    };
    let root_ancestor = dex.chain.state().compute_root();

    // Branch A: swap + swap.
    dex.chain
        .calls_v1_block(vec![swap_call(&dex, dex.alice, dex.za, 1_000_000)])
        .expect("A1");
    dex.chain
        .calls_v1_block(vec![swap_call(&dex, dex.bob, dex.za, 200_000)])
        .expect("A2");
    let root_a = dex.chain.state().compute_root();

    // Reorg back to the ancestor via the REAL platform rollback.
    dex.chain
        .state_mut()
        .rollback_to(ancestor_height)
        .expect("rollback");
    assert_eq!(dex.chain.state().compute_root(), root_ancestor);
    dex.chain.set_height(ancestor_height);

    // Branch B: a different swap.
    dex.chain
        .calls_v1_block(vec![swap_call(&dex, dex.alice, dex.zb, 3_000_000)])
        .expect("B1");
    let root_b = dex.chain.state().compute_root();
    assert_ne!(root_a, root_b);

    // Clean reindex: a pristine chain replaying setup + branch B matches.
    let mut fresh = setup();
    fresh
        .chain
        .calls_v1_block(vec![swap_call(&fresh, fresh.alice, fresh.zb, 3_000_000)])
        .expect("B1 replay");
    assert_eq!(
        fresh.chain.state().compute_root(),
        root_b,
        "reorged chain == clean reindex"
    );
}

#[test]
fn two_independent_nodes_equal_roots_per_block() {
    // The same deterministic script on two isolated chains must agree on
    // the root after EVERY block (not just the final one). setup() +
    // three swap blocks, with per-step comparison.
    let script = |dex: &mut Dex, roots: &mut Vec<[u8; 32]>| {
        for (signer, asset, amount) in [
            (dex.alice, dex.za, 1_000_000u128),
            (dex.bob, dex.za, 250_000),
            (dex.alice, dex.zb, 4_000_000),
        ] {
            let records = dex
                .chain
                .calls_v1_block(vec![swap_call(dex, signer, asset, amount)])
                .expect("swap block");
            assert!(records[0].success);
            roots.push(dex.chain.state().compute_root().0);
        }
    };
    let mut node_a = setup();
    let mut node_b = setup();
    assert_eq!(
        node_a.chain.state().compute_root(),
        node_b.chain.state().compute_root(),
        "identical roots after setup"
    );
    let (mut roots_a, mut roots_b) = (Vec::new(), Vec::new());
    script(&mut node_a, &mut roots_a);
    script(&mut node_b, &mut roots_b);
    assert_eq!(roots_a, roots_b, "roots equal at every height");
}

#[test]
fn malformed_v1_input_is_deterministic_and_atomic() {
    let mut dex = setup();
    let root = dex.chain.state().compute_root();

    // Malformed calldata to a real contract: deterministic error code 18,
    // zero state residue.
    let record = dex
        .chain
        .call_v1(V1Call::new(dex.pool, pool_op::SWAP_EXACT_IN, vec![0xab; 7]))
        .expect("block");
    assert!(!record.success);
    assert!(record.error.as_deref().unwrap_or("").contains("code 18"));
    assert_eq!(dex.chain.state().compute_root(), root);

    // V1 call to a nonexistent contract: recorded, failed, no residue.
    let record = dex
        .chain
        .call_v1(V1Call::new(ContractId([0xEE; 32]), 7, vec![]))
        .expect("block");
    assert!(!record.success);
    assert_eq!(record.error.as_deref(), Some("contract not found"));
    assert_eq!(dex.chain.state().compute_root(), root);

    // Unknown opcode: contract-level InvalidOpcode (17), atomic.
    let record = dex
        .chain
        .call_v1(V1Call::new(dex.pool, 555, vec![]))
        .expect("block");
    assert!(!record.success);
    assert!(record.error.as_deref().unwrap_or("").contains("code 17"));
    assert_eq!(dex.chain.state().compute_root(), root);
}

#[test]
fn views_and_details_work_on_the_real_engine() {
    let dex = setup();
    let details = decode_pool_details(
        &dex.chain
            .view_v1(dex.pool, pool_op::POOL_DETAILS, &[])
            .expect("details"),
    )
    .expect("decode");
    assert_eq!(details.pool_id, dex.pool);
    assert_eq!(details.factory_id, dex.factory);
    assert_eq!(details.total_lp_supply, 10_000_000);
    let name = dex
        .chain
        .view_v1(dex.pool, pool_op::GET_NAME, &[])
        .expect("name");
    let name = String::from_utf8(name).expect("utf8");
    assert!(name.ends_with(" LP"), "{name}");
    assert!(name.contains("Zalkanes Test Asset"), "{name}");
}

/// Keep the borrow checker happy for Execution comparisons.
#[allow(dead_code)]
fn assert_execution_eq(a: &Execution, b: &Execution) {
    assert_eq!(a, b);
}

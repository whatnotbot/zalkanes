//! PROTOCOL V1 execution vectors (ADR-0008 §18): a fixed V1 scenario —
//! deploys, mints, factory pool creation (spawn), a two-swap block, a
//! failing swap — with the state root and per-message fuel pinned after
//! every block. Any consensus drift in the V1 engine, the asset-ledger
//! root, or the dex contracts' WASM fails this test.
//!
//! Regenerate with ZALKANES_GENERATE_VECTORS=1.

use zalkanes_core::types::{CodeHash, ContractId, Network};
use zalkanes_dex_core::encode::{
    factory_create_pool_args, factory_initialize_args, factory_op, pool_op, pool_swap_args,
    token_initialize_args, token_mint_args, token_op,
};
use zalkanes_dex_core::types::{AssetId, Holder};
use zalkanes_state::StateStore;
use zalkanes_testkit::{external_holder_for, test_signer, TestChain, V1Call};

const TOKEN_WASM: &[u8] = include_bytes!("../fixtures/test_token.wasm");
const POOL_WASM: &[u8] = include_bytes!("../fixtures/subfrost_pool.wasm");
const FACTORY_WASM: &[u8] = include_bytes!("../fixtures/subfrost_factory.wasm");

const VECTOR_PATH: &str = "../../test-vectors/execution/v2-dex.json";

fn run_scenario() -> Vec<serde_json::Value> {
    let mut steps = Vec::new();
    let mut chain = TestChain::new();
    let mut record = |name: &str, chain: &TestChain, fuel: Vec<u64>, success: Vec<bool>| {
        steps.push(serde_json::json!({
            "step": name,
            "height": chain.height(),
            "root": format!("{}", chain.state_root()),
            "fuel": fuel,
            "success": success,
        }));
    };

    chain.mine_empty_block().unwrap();
    record("pre-v1-block", &chain, vec![], vec![]);

    let token_a = chain.deploy_v1(TOKEN_WASM).unwrap();
    let token_b = chain.deploy_v1(TOKEN_WASM).unwrap();
    let factory = chain.deploy_v1(FACTORY_WASM).unwrap();
    let _template = chain.deploy_v1(POOL_WASM).unwrap();
    record("deploys", &chain, vec![], vec![]);

    let alice = test_signer(1);
    let bob = test_signer(2);
    let alice_holder = external_holder_for(Network::Regtest, &alice);
    let bob_holder = external_holder_for(Network::Regtest, &bob);
    let holder = |raw: &[u8; 33]| Holder::from_bytes(raw).unwrap();

    let mut fuels = Vec::new();
    let mut oks = Vec::new();
    for (token, name) in [
        (token_a, "Zalkanes Test Asset A"),
        (token_b, "Zalkanes Test Asset B"),
    ] {
        let r = chain
            .call_v1(V1Call::new(
                token,
                token_op::INITIALIZE,
                token_initialize_args(name),
            ))
            .unwrap();
        fuels.push(r.fuel_used);
        oks.push(r.success);
    }
    for (token, to, amount) in [
        (token_a, alice_holder, 1_000_000_000u128),
        (token_b, alice_holder, 1_000_000_000),
        (token_a, bob_holder, 50_000_000),
    ] {
        let r = chain
            .call_v1(V1Call::new(
                token,
                token_op::MINT_FOR_TEST,
                token_mint_args(&holder(&to), amount),
            ))
            .unwrap();
        fuels.push(r.fuel_used);
        oks.push(r.success);
    }
    record("token-setup", &chain, fuels, oks);

    let r = chain
        .call_v1(V1Call::new(
            factory,
            factory_op::INITIALIZE,
            factory_initialize_args(&CodeHash::of(POOL_WASM).0),
        ))
        .unwrap();
    let mut create = V1Call::new(
        factory,
        factory_op::CREATE_POOL,
        factory_create_pool_args(&AssetId(token_a.0), &AssetId(token_b.0)),
    );
    create.attached = vec![(token_a.0, 5_000_000), (token_b.0, 20_000_000)];
    create.signer = Some(alice);
    let r2 = chain.call_v1(create).unwrap();
    assert!(r2.success, "pool creation must succeed: {:?}", r2.error);
    let mut pool_id = [0u8; 32];
    pool_id.copy_from_slice(&r2.return_data);
    let pool = ContractId(pool_id);
    record(
        "factory-and-pool",
        &chain,
        vec![r.fuel_used, r2.fuel_used],
        vec![r.success, r2.success],
    );

    let swap = |signer: secp256k1::SecretKey, asset: [u8; 32], amount: u128, min_out: u128| {
        let mut call = V1Call::new(pool, pool_op::SWAP_EXACT_IN, pool_swap_args(min_out, 0));
        call.attached = vec![(asset, amount)];
        call.signer = Some(signer);
        call
    };

    // Two swaps in ONE block (sequential visibility) + a failing swap block.
    let records = chain
        .calls_v1_block(vec![
            swap(alice, token_a.0, 1_000_000, 0),
            swap(bob, token_a.0, 1_000_000, 0),
        ])
        .unwrap();
    record(
        "two-swap-block",
        &chain,
        records.iter().map(|r| r.fuel_used).collect(),
        records.iter().map(|r| r.success).collect(),
    );

    let records = chain
        .calls_v1_block(vec![swap(bob, token_a.0, 500_000, u128::MAX)])
        .unwrap();
    assert!(!records[0].success);
    record(
        "failing-swap-block",
        &chain,
        records.iter().map(|r| r.fuel_used).collect(),
        records.iter().map(|r| r.success).collect(),
    );

    // Ledger digest: every row, canonical order.
    let mut rows = Vec::new();
    for ((holder_bytes, asset), amount) in &chain.state().ledger {
        rows.push(serde_json::json!({
            "holder": hex::encode(holder_bytes),
            "asset": hex::encode(asset),
            "amount": amount.to_string(),
        }));
    }
    steps.push(serde_json::json!({ "step": "final-ledger", "rows": rows }));
    steps
}

#[test]
fn v2_execution_vectors_match_committed_json() {
    let current = serde_json::to_string_pretty(&serde_json::json!({
        "schema": "zalkanes-execution-v2-dex-vectors",
        "steps": run_scenario(),
    }))
    .unwrap();
    if std::env::var("ZALKANES_GENERATE_VECTORS").is_ok() {
        std::fs::write(VECTOR_PATH, format!("{current}\n")).unwrap();
        return;
    }
    let committed = std::fs::read_to_string(VECTOR_PATH)
        .expect("committed vectors exist (regenerate with ZALKANES_GENERATE_VECTORS=1)");
    assert_eq!(
        committed.trim_end(),
        current,
        "v2 execution vectors drifted"
    );
}

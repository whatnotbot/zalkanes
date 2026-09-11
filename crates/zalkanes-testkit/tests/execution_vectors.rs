//! §16.10 state-root determinism vectors: a deterministic 100+-block scenario
//! driven through the REAL production block processor, with the root and the
//! execution outcomes (success, fuel, error class) recorded per block.
//!
//! Verify mode (default): replay the scenario and require byte-identical
//! roots/outcomes against `test-vectors/execution/v1.json`.
//! Generate mode: `ZALKANES_GENERATE_VECTORS=1 cargo test -p zalkanes-testkit
//! --test execution_vectors` rewrites the vector file.
//!
//! Scenario coverage: deploy, repeated calls, initialize with input, unknown
//! opcode (deterministic error trap), call to a missing contract, malformed
//! ZALK payloads (parser skip/error paths), fuel exhaustion (infinite loop),
//! trap-after-write (writes discarded), storage writes + same-call
//! write-then-delete, a second instance of identical code, multi-transaction
//! blocks (intra-block visibility semantics locked in), no-op blocks, and a
//! rollback + fresh-replay equality check.
//!
//! Nested cross-contract calls do not exist in protocol v0 (no `contract_call`
//! host ABI); that row of the matrix activates with the ABI in a future
//! protocol version.

use serde::{Deserialize, Serialize};
use zalkanes_core::types::ContractId;
use zalkanes_state::StateStore;
use zalkanes_testkit::TestChain;

const VECTOR_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../test-vectors/execution/v1.json"
);

#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
struct ExecutionVector {
    success: bool,
    fuel_used: u64,
    error: Option<String>,
}

#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
struct BlockVector {
    height: u32,
    desc: String,
    root: String,
    executions: Vec<ExecutionVector>,
}

fn counter_wasm() -> Vec<u8> {
    std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/fixtures/counter.wasm"
    ))
    .expect("counter fixture")
}

fn spin_wasm() -> Vec<u8> {
    wat::parse_str(
        r#"(module
            (memory (export "memory") 1)
            (func (export "dispatch") (param i32 i32) (result i32)
                (loop $l (br $l))
                i32.const 0))"#,
    )
    .unwrap()
}

fn store_delete_wasm() -> Vec<u8> {
    // opcode 1: write key "a"; opcode 2: write then delete "a" in one call;
    // opcode 3: write "a" then unreachable (writes must be discarded).
    wat::parse_str(
        r#"(module
            (import "env" "storage_set" (func $set (param i32 i32 i32 i32) (result i32)))
            (import "env" "storage_delete" (func $del (param i32 i32) (result i32)))
            (memory (export "memory") 1)
            (data (i32.const 0) "a")
            (func (export "dispatch") (param $op i32) (param $len i32) (result i32)
                (if (i32.eq (local.get $op) (i32.const 1))
                    (then (drop (call $set (i32.const 0) (i32.const 1) (i32.const 0) (i32.const 1)))
                          (return (i32.const 0))))
                (if (i32.eq (local.get $op) (i32.const 2))
                    (then (drop (call $set (i32.const 0) (i32.const 1) (i32.const 0) (i32.const 1)))
                          (drop (call $del (i32.const 0) (i32.const 1)))
                          (return (i32.const 0))))
                (if (i32.eq (local.get $op) (i32.const 3))
                    (then (drop (call $set (i32.const 0) (i32.const 1) (i32.const 0) (i32.const 1)))
                          unreachable))
                i32.const 0))"#,
    )
    .unwrap()
}

fn record(chain: &TestChain, desc: &str, out: &mut Vec<BlockVector>) {
    let height = chain.height();
    let executions = chain
        .state()
        .executions_at_height(height)
        .into_iter()
        .map(|e| ExecutionVector {
            success: e.success,
            fuel_used: e.fuel_used,
            error: e.error,
        })
        .collect();
    out.push(BlockVector {
        height,
        desc: desc.to_string(),
        root: chain
            .state_root()
            .0
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
        executions,
    });
}

/// Build the full deterministic scenario; returns per-block vectors.
fn run_scenario() -> Vec<BlockVector> {
    let mut out = Vec::new();
    let mut chain = TestChain::new();

    let counter = chain.deploy(&counter_wasm()).unwrap();
    record(&chain, "deploy counter", &mut out);

    for i in 0..60 {
        chain.call(counter, 1, &[]).unwrap();
        record(&chain, &format!("increment #{}", i + 1), &mut out);
    }

    chain.call(counter, 3, &7u64.to_be_bytes()).unwrap();
    record(&chain, "initialize(7)", &mut out);

    for i in 0..10 {
        chain.call(counter, 1, &[]).unwrap();
        record(&chain, &format!("post-init increment #{}", i + 1), &mut out);
    }

    chain.call(counter, 0x999, &[]).unwrap();
    record(&chain, "unknown opcode (deterministic error)", &mut out);

    chain.call(ContractId([0xAB; 32]), 1, &[]).unwrap();
    record(&chain, "call to missing contract", &mut out);

    // Malformed / foreign payloads: parser skip and error paths.
    chain.raw_message_block(b"ZALK").unwrap();
    record(&chain, "truncated magic-only payload", &mut out);
    chain
        .raw_message_block(&[0x5a, 0x41, 0x4c, 0x4b, 0xEE, 0x02])
        .unwrap();
    record(&chain, "unknown version byte", &mut out);
    chain
        .raw_message_block(b"unrelated op_return data")
        .unwrap();
    record(&chain, "non-ZALK payload skipped", &mut out);

    let spinner = chain.deploy(&spin_wasm()).unwrap();
    record(&chain, "deploy infinite-loop contract", &mut out);
    chain.call(spinner, 1, &[]).unwrap();
    record(&chain, "fuel exhaustion (spin)", &mut out);

    let store = chain.deploy(&store_delete_wasm()).unwrap();
    record(&chain, "deploy store/delete contract", &mut out);
    chain.call(store, 1, &[]).unwrap();
    record(&chain, "storage write", &mut out);
    chain.call(store, 2, &[]).unwrap();
    record(&chain, "same-call write-then-delete", &mut out);
    chain.call(store, 3, &[]).unwrap();
    record(&chain, "trap after write (writes discarded)", &mut out);

    let counter2 = chain.deploy(&counter_wasm()).unwrap();
    assert_ne!(counter, counter2, "same code, new txid, distinct identity");
    record(&chain, "deploy second counter instance", &mut out);
    for i in 0..10 {
        chain.call(counter2, 1, &[]).unwrap();
        record(&chain, &format!("counter2 increment #{}", i + 1), &mut out);
    }

    // Multi-transaction blocks: v0 intra-block semantics (each call reads the
    // pre-block state) are consensus behavior and are locked in here.
    chain
        .multi_call_block(&[
            (counter, 1, vec![]),
            (counter, 1, vec![]),
            (counter2, 2, vec![]),
        ])
        .unwrap();
    record(&chain, "multi-tx block (2x increment + get)", &mut out);
    chain
        .multi_call_block(&[(counter, 2, vec![]), (ContractId([0xCD; 32]), 1, vec![])])
        .unwrap();
    record(&chain, "multi-tx block (get + missing contract)", &mut out);

    for i in 0..5 {
        chain.mine_empty_block().unwrap();
        record(&chain, &format!("no-op block #{}", i + 1), &mut out);
    }

    // Rollback / reapply: rolling back to a recorded height must reproduce
    // that height's recorded root exactly.
    let mid = out[out.len() / 2].clone();
    chain.state_mut().rollback_to(mid.height).unwrap();
    assert_eq!(
        chain
            .state()
            .compute_root()
            .0
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>(),
        mid.root,
        "rollback to height {} must reproduce its recorded root",
        mid.height
    );

    out
}

#[test]
fn execution_vectors_are_reproduced_exactly() {
    let vectors = run_scenario();
    assert!(
        vectors.len() >= 100,
        "scenario must cover >= 100 blocks, got {}",
        vectors.len()
    );

    if std::env::var("ZALKANES_GENERATE_VECTORS").as_deref() == Ok("1") {
        std::fs::write(VECTOR_PATH, serde_json::to_string_pretty(&vectors).unwrap()).unwrap();
        eprintln!("wrote {} vectors to {VECTOR_PATH}", vectors.len());
        return;
    }

    let committed: Vec<BlockVector> = serde_json::from_str(
        &std::fs::read_to_string(VECTOR_PATH)
            .expect("committed vectors present (generate with ZALKANES_GENERATE_VECTORS=1)"),
    )
    .unwrap();
    assert_eq!(
        vectors.len(),
        committed.len(),
        "vector count changed — consensus behavior drifted"
    );
    for (got, want) in vectors.iter().zip(&committed) {
        assert_eq!(got, want, "vector mismatch at height {}", want.height);
    }
}

#[test]
fn fresh_replay_is_bit_identical() {
    // Two independent replays of the whole scenario must agree everywhere.
    let a = run_scenario();
    let b = run_scenario();
    assert_eq!(a, b);
}

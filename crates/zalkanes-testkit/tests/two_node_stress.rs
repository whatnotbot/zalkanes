//! §16.11 two-independent-replay stress at target scale, through the REAL
//! production block processor: 100+ deployments, 10,000+ successful calls,
//! 1,000+ intentional failures. Two fully independent replays (separate
//! state databases) must produce byte-identical roots at EVERY block and
//! identical execution outcomes.
//!
//! Scope note: this is the deterministic in-process form of the two-node
//! test (same chain, two isolated DBs). The live two-STACK form ran against
//! public testnet: an independent local indexer resynced the whole chain
//! from activation and reproduced the Railway indexer's root byte-for-byte
//! at height 4,339,534 (see audit evidence).

use zalkanes_core::types::ContractId;
use zalkanes_state::StateStore;
use zalkanes_testkit::TestChain;

const DEPLOYS: usize = 100;
const CALL_BLOCKS: usize = 500;
const CALLS_PER_BLOCK: usize = 20; // 10,000 successful calls
const FAILURE_BLOCKS: usize = 50;
const FAILURES_PER_BLOCK: usize = 20; // 1,000 intentional failures

fn counter_wasm() -> Vec<u8> {
    std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/fixtures/counter.wasm"
    ))
    .expect("counter fixture")
}

struct ReplaySummary {
    roots: Vec<(u32, String)>,
    successes: u64,
    failures: u64,
}

fn replay() -> ReplaySummary {
    let wasm = counter_wasm();
    let mut chain = TestChain::new();
    let mut roots = Vec::new();
    let record = |c: &TestChain, roots: &mut Vec<(u32, String)>| {
        roots.push((
            c.height(),
            c.state_root()
                .0
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect(),
        ));
    };

    // >= 100 deployments (same code, distinct identities).
    let mut contracts = Vec::with_capacity(DEPLOYS);
    for _ in 0..DEPLOYS {
        contracts.push(chain.deploy(&wasm).unwrap());
        record(&chain, &mut roots);
    }

    // >= 10,000 successful calls, batched into multi-transaction blocks and
    // spread across every contract.
    for b in 0..CALL_BLOCKS {
        let calls: Vec<(ContractId, u16, Vec<u8>)> = (0..CALLS_PER_BLOCK)
            .map(|i| (contracts[(b * CALLS_PER_BLOCK + i) % DEPLOYS], 1u16, vec![]))
            .collect();
        chain.multi_call_block(&calls).unwrap();
        record(&chain, &mut roots);
    }

    // >= 1,000 intentional failures: unknown opcodes and missing contracts.
    for b in 0..FAILURE_BLOCKS {
        let calls: Vec<(ContractId, u16, Vec<u8>)> = (0..FAILURES_PER_BLOCK)
            .map(|i| {
                if (b + i) % 2 == 0 {
                    (contracts[i % DEPLOYS], 0x666u16, vec![]) // unknown opcode
                } else {
                    (ContractId([0xEE; 32]), 1u16, vec![]) // missing contract
                }
            })
            .collect();
        chain.multi_call_block(&calls).unwrap();
        record(&chain, &mut roots);
    }

    // Tally execution outcomes across the whole chain.
    let mut successes = 0u64;
    let mut failures = 0u64;
    for h in 1..=chain.height() {
        for e in chain.state().executions_at_height(h) {
            if e.success {
                successes += 1;
            } else {
                failures += 1;
            }
        }
    }
    ReplaySummary {
        roots,
        successes,
        failures,
    }
}

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "full scale runs in release (CI runs cargo test --release)"
)]
fn two_independent_replays_agree_at_scale() {
    let a = replay();
    let b = replay();

    assert!(a.successes >= 10_000, "successful calls: {}", a.successes);
    assert!(a.failures >= 1_000, "intentional failures: {}", a.failures);
    assert_eq!(a.roots.len(), b.roots.len());
    for ((ha, ra), (hb, rb)) in a.roots.iter().zip(&b.roots) {
        assert_eq!(ha, hb);
        assert_eq!(ra, rb, "root divergence at height {ha}");
    }
    assert_eq!(a.successes, b.successes);
    assert_eq!(a.failures, b.failures);
}

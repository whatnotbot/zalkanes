//! §16.9 WASM adversarial suite: forbidden proposals, resource boundaries,
//! fuel/traps, recursion, storage amplification, and import escape attempts.
//! Every case requires a deterministic accept/reject and — where execution
//! happens — identical fuel, return data, and writes across repeated runs.

use zalkanes_core::types::{BlockHash, BlockHeight, CodeHash, ContractId, TxId};
use zalkanes_runtime::{execute, validate_module, CallContext, CallResult};
use zalkanes_state::{BlockCommit, MemoryState, StateStore};

fn wat(src: &str) -> Vec<u8> {
    wat::parse_str(src).expect("valid wat")
}

fn deployed(wasm: &[u8]) -> (MemoryState, ContractId) {
    let id = ContractId([0x11; 32]);
    let mut state = MemoryState::new();
    state
        .commit_block(BlockCommit {
            height: 1,
            zcash_block_hash: BlockHash([0x22; 32]),
            deploys: vec![(id, CodeHash::of(wasm), wasm.to_vec())],
            upserts: vec![],
            deletes: vec![],
        })
        .unwrap();
    (state, id)
}

fn call(state: &MemoryState, id: ContractId, opcode: u16, fuel: u64) -> CallResult {
    execute(
        CallContext {
            contract_id: id,
            caller: None,
            txid: TxId([0x33; 32]),
            block_height: BlockHeight::from(2u32),
            opcode,
            input: vec![],
            fuel_limit: fuel,
            depth: 0,
        },
        state,
    )
}

/// Two executions of the same call must be byte-identical in every observable
/// dimension (fuel, output, writes, failure reason).
fn assert_deterministic(state: &MemoryState, id: ContractId, opcode: u16, fuel: u64) -> CallResult {
    let a = call(state, id, opcode, fuel);
    let b = call(state, id, opcode, fuel);
    match (&a, &b) {
        (
            CallResult::Success {
                output: o1,
                fuel_used: f1,
                writes: w1,
            },
            CallResult::Success {
                output: o2,
                fuel_used: f2,
                writes: w2,
            },
        ) => {
            assert_eq!(o1, o2, "output determinism");
            assert_eq!(f1, f2, "fuel determinism");
            assert_eq!(w1, w2, "writes determinism");
        }
        (
            CallResult::Trap {
                reason: r1,
                fuel_used: f1,
            },
            CallResult::Trap {
                reason: r2,
                fuel_used: f2,
            },
        ) => {
            assert_eq!(r1, r2, "trap reason determinism");
            assert_eq!(f1, f2, "trap fuel determinism");
        }
        (
            CallResult::FuelExhausted { fuel_used: f1 },
            CallResult::FuelExhausted { fuel_used: f2 },
        ) => {
            assert_eq!(f1, f2, "exhaustion fuel determinism");
        }
        (x, y) => panic!("nondeterministic result classes: {x:?} vs {y:?}"),
    }
    a
}

// ── Forbidden proposals / structure ──────────────────────────────────────────

#[test]
fn float_instructions_are_rejected_at_validation() {
    let m = wat(r#"(module
        (memory (export "memory") 1)
        (func (export "dispatch") (param i32 i32) (result i32)
            f32.const 1.5
            f32.const 2.5
            f32.add
            i32.trunc_f32_s))"#);
    let err = validate_module(&m).expect_err("floats are forbidden");
    assert!(!err.is_empty());
}

#[test]
fn float_types_in_signatures_are_rejected() {
    let m = wat(r#"(module
        (memory (export "memory") 1)
        (func $f (param f64) (result f64) local.get 0)
        (func (export "dispatch") (param i32 i32) (result i32) i32.const 0))"#);
    assert!(
        validate_module(&m).is_err(),
        "f64 signature must be rejected"
    );
}

#[test]
fn start_function_is_rejected() {
    let m = wat(r#"(module
        (memory (export "memory") 1)
        (func $init)
        (start $init)
        (func (export "dispatch") (param i32 i32) (result i32) i32.const 0))"#);
    assert!(
        validate_module(&m).is_err(),
        "start functions are forbidden"
    );
}

#[test]
fn memory64_is_rejected() {
    let m = wat(r#"(module
        (memory (export "memory") i64 1)
        (func (export "dispatch") (param i32 i32) (result i32) i32.const 0))"#);
    assert!(validate_module(&m).is_err(), "memory64 is forbidden");
}

#[test]
fn unknown_env_import_is_rejected_at_validation() {
    // The cross-contract-call ABI does not exist in v0; importing it must be
    // a validation failure, not a deploy-then-brick.
    let m = wat(r#"(module
        (import "env" "contract_call" (func (param i32 i32 i32 i32) (result i32)))
        (memory (export "memory") 1)
        (func (export "dispatch") (param i32 i32) (result i32) i32.const 0))"#);
    let err = validate_module(&m).expect_err("unknown import must be rejected");
    assert!(err.contains("unresolvable import"), "got: {err}");
}

#[test]
fn wasi_and_ambient_capability_imports_are_rejected() {
    for (module, name, ty) in [
        (
            "wasi_snapshot_preview1",
            "fd_write",
            "(param i32 i32 i32 i32) (result i32)",
        ),
        (
            "wasi_snapshot_preview1",
            "random_get",
            "(param i32 i32) (result i32)",
        ),
        (
            "wasi_snapshot_preview1",
            "clock_time_get",
            "(param i32 i64 i32) (result i32)",
        ),
        ("env", "gettimeofday", "(param i32) (result i32)"),
        ("env", "getenv", "(param i32) (result i32)"),
    ] {
        let m = wat(&format!(
            r#"(module
                (import "{module}" "{name}" (func {ty}))
                (memory (export "memory") 1)
                (func (export "dispatch") (param i32 i32) (result i32) i32.const 0))"#
        ));
        assert!(
            validate_module(&m).is_err(),
            "{module}::{name} must be rejected at validation"
        );
    }
}

#[test]
fn import_with_wrong_kind_is_rejected() {
    let m = wat(r#"(module
        (import "env" "storage_get" (global i32))
        (memory (export "memory") 1)
        (func (export "dispatch") (param i32 i32) (result i32) i32.const 0))"#);
    assert!(
        validate_module(&m).is_err(),
        "non-function import of a host name"
    );
}

// ── Resource boundaries ──────────────────────────────────────────────────────

#[test]
fn memory_growth_beyond_cap_fails_deterministically_in_wasm() {
    // dispatch(): grow 25 pages at a time, 100 times; return the number of
    // successful grows. The consensus cap (256 pages) must make later grows
    // fail with -1 deterministically — never host OOM.
    let m = wat(r#"(module
        (memory (export "memory") 1)
        (func (export "dispatch") (param i32 i32) (result i32)
            (local $i i32) (local $ok i32)
            (block $done
              (loop $l
                (br_if $done (i32.ge_u (local.get $i) (i32.const 100)))
                (if (i32.ne (memory.grow (i32.const 25)) (i32.const -1))
                    (then (local.set $ok (i32.add (local.get $ok) (i32.const 1)))))
                (local.set $i (i32.add (local.get $i) (i32.const 1)))
                (br $l)))
            local.get $ok))"#);
    validate_module(&m).expect("declares 1 page; growth is a runtime matter");
    let (state, id) = deployed(&m);
    // A nonzero dispatch return is reported as a deterministic error trap,
    // which conveniently carries the grow count: 1 initial page + 10 grows
    // of 25 pages = 251; the 11th (276 > 256-page cap) must fail with -1.
    let result = assert_deterministic(&state, id, 0, 50_000_000);
    match result {
        CallResult::Trap { reason, .. } => {
            assert!(
                reason.contains("error code 10"),
                "exactly 10 grows must succeed under the 256-page cap: {reason}"
            );
        }
        other => panic!("expected deterministic capped growth, got {other:?}"),
    }
}

#[test]
fn recursion_exhausts_deterministically() {
    let m = wat(r#"(module
        (memory (export "memory") 1)
        (func $r (param i32) (result i32)
            (i32.add (local.get 0) (call $r (i32.add (local.get 0) (i32.const 1)))))
        (func (export "dispatch") (param i32 i32) (result i32)
            (call $r (i32.const 0))))"#);
    validate_module(&m).expect("structurally valid");
    let (state, id) = deployed(&m);
    let result = assert_deterministic(&state, id, 0, 100_000_000);
    assert!(
        matches!(
            result,
            CallResult::Trap { .. } | CallResult::FuelExhausted { .. }
        ),
        "unbounded recursion must trap or exhaust: {result:?}"
    );
}

#[test]
fn infinite_loop_exhausts_exact_fuel() {
    let m = wat(r#"(module
        (memory (export "memory") 1)
        (func (export "dispatch") (param i32 i32) (result i32)
            (loop $l (br $l))
            i32.const 0))"#);
    let (state, id) = deployed(&m);
    let result = assert_deterministic(&state, id, 0, 1_000_000);
    match result {
        CallResult::FuelExhausted { fuel_used } => {
            assert_eq!(fuel_used, 1_000_000, "exhaustion consumes the whole limit")
        }
        other => panic!("expected FuelExhausted, got {other:?}"),
    }
}

#[test]
fn trap_discards_writes() {
    // Write a key, then hit unreachable: the write must not surface.
    let m = wat(r#"(module
        (import "env" "storage_set" (func $set (param i32 i32 i32 i32) (result i32)))
        (memory (export "memory") 1)
        (data (i32.const 0) "kv")
        (func (export "dispatch") (param i32 i32) (result i32)
            (drop (call $set (i32.const 0) (i32.const 1) (i32.const 1) (i32.const 1)))
            unreachable))"#);
    validate_module(&m).expect("valid module");
    let (state, id) = deployed(&m);
    let result = assert_deterministic(&state, id, 0, 10_000_000);
    match result {
        CallResult::Trap { .. } => {} // structurally: Trap carries no writes
        other => panic!("expected trap, got {other:?}"),
    }
}

#[test]
fn storage_write_amplification_is_capped() {
    // Try to create far more distinct keys than MAX_STORAGE_WRITES_PER_CALL;
    // the host must refuse past the cap (-1) and the call stays deterministic.
    let m = wat(r#"(module
        (import "env" "storage_set" (func $set (param i32 i32 i32 i32) (result i32)))
        (memory (export "memory") 1)
        (func (export "dispatch") (param i32 i32) (result i32)
            (local $i i32) (local $ok i32)
            (block $done
              (loop $l
                (br_if $done (i32.ge_u (local.get $i) (i32.const 5000)))
                ;; 4-byte key = loop counter stored at 0
                (i32.store (i32.const 0) (local.get $i))
                (if (i32.eq (call $set (i32.const 0) (i32.const 4) (i32.const 0) (i32.const 4)) (i32.const 0))
                    (then (local.set $ok (i32.add (local.get $ok) (i32.const 1)))))
                (local.set $i (i32.add (local.get $i) (i32.const 1)))
                (br $l)))
            local.get $ok))"#);
    validate_module(&m).expect("valid module");
    let (state, id) = deployed(&m);
    let cap = zalkanes_core::consensus::MAX_STORAGE_WRITES_PER_CALL;
    let result = assert_deterministic(&state, id, 0, 500_000_000);
    match result {
        CallResult::Trap { reason, .. } => {
            assert!(
                reason.contains(&format!("error code {cap}")),
                "exactly the per-call cap ({cap}) of writes must succeed: {reason}"
            );
        }
        other => panic!("expected deterministic capped writes, got {other:?}"),
    }
}

#[test]
fn negative_pointer_host_calls_fail_without_panicking() {
    // Host functions receiving hostile pointers must return errors, never
    // panic the node (a deterministic panic would halt every indexer).
    let m = wat(r#"(module
        (import "env" "storage_get" (func $get (param i32 i32 i32) (result i32)))
        (import "env" "input_read" (func $inp (param i32 i32 i32) (result i32)))
        (import "env" "context_block_height" (func $h (param i32) (result i32)))
        (memory (export "memory") 1)
        (data (i32.const 0) "k")
        (func (export "dispatch") (param i32 i32) (result i32)
            (drop (call $get (i32.const 0) (i32.const 1) (i32.const -1)))
            (drop (call $inp (i32.const -1) (i32.const 0) (i32.const 4)))
            (drop (call $h (i32.const -1)))
            i32.const 0))"#);
    validate_module(&m).expect("valid module");
    let (state, id) = deployed(&m);
    let result = assert_deterministic(&state, id, 0, 10_000_000);
    assert!(
        matches!(result, CallResult::Success { .. }),
        "hostile pointers must yield host-fn error codes, not a panic: {result:?}"
    );
}

// ── Live-chain regression anchor ─────────────────────────────────────────────

#[test]
fn deployed_counter_fixture_still_validates_and_runs_with_live_fuel() {
    // The exact WASM deployed on public testnet (code hash fa8289fb..) must
    // remain valid under the hardened configuration, and `increment` must
    // consume exactly the fuel recorded in the live execution records (3936).
    let wasm = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../zalkanes-testkit/fixtures/counter.wasm"
    ))
    .expect("counter fixture");
    validate_module(&wasm).expect("live counter must remain valid");

    let (state, id) = deployed(&wasm);
    let result = assert_deterministic(&state, id, 1, 10_000_000);
    match result {
        CallResult::Success { fuel_used, .. } => {
            assert_eq!(
                fuel_used, 3936,
                "live-recorded increment fuel must be reproduced exactly"
            );
        }
        other => panic!("increment must succeed: {other:?}"),
    }
}

#[test]
fn wasmi_translator_panic_input_is_rejected_not_fatal() {
    // Found by the wasm_validator fuzzer: this module makes the pinned wasmi
    // 2.0.0 translator PANIC internally (control.rs stack assertion). For a
    // consensus node a deterministic panic is a network-wide halt, so
    // validate_module contains the panic and rejects the module — twice, to
    // prove the rejection is deterministic.
    let bytes = include_bytes!("fixtures/wasmi_translator_panic.bin");
    let a = validate_module(bytes).expect_err("must be rejected, not a panic");
    let b = validate_module(bytes).expect_err("must be rejected, not a panic");
    assert_eq!(a, b, "containment must be deterministic");
    // Profile note: debug builds hit a wasmi-internal assertion (contained
    // into "translation panic"); release builds reject with a clean parse
    // error. The CONSENSUS outcome — deterministic rejection — is identical
    // in both profiles and on both architectures.
    assert!(a.starts_with("WASM parse error"), "got: {a}");
}

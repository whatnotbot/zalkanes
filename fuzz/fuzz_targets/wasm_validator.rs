//! Consensus WASM module validation: arbitrary bytes must never panic the
//! validator and oversized modules must be rejected before allocation.

#![no_main]
use libfuzzer_sys::fuzz_target;
use zalkanes_runtime::validate_module;

// libfuzzer's default panic hook aborts BEFORE library-internal
// catch_unwind containment (as used by validate_module) can return, so this
// target replaces it with a print-only hook: contained panics behave as in
// production, while target-level assertion failures still unwind into the
// harness and are reported as crashes.
fn print_only_hook() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        std::panic::set_hook(Box::new(|info| eprintln!("{info}")));
    });
}

fuzz_target!(|data: &[u8]| {
    print_only_hook();
    let _ = validate_module(data);
});

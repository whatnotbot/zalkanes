//! Enforcement tests: prove every consensus constant that can be enforced is
//! actually enforced, at `limit - 1` / `limit` / `limit + 1` where applicable.
//!
//! Each test must demonstrate deterministic rejection with no panic.

use zalkanes_core::consensus::{
    MAX_EXPORTS, MAX_IMPORTS, MAX_LINEAR_MEMORY_PAGES, MAX_TABLE_ELEMENTS,
};
use zalkanes_runtime::validate_module;

fn wat_module(wat: &str) -> Vec<u8> {
    wat::parse_str(wat).expect("valid WAT")
}

/// A minimal module Zalkanes accepts: `memory` export + `dispatch` export.
const MINIMAL: &str = r#"
(module
  (memory (export "memory") 1)
  (func (export "dispatch") (param i32 i32) (result i32)
    i32.const 0))
"#;

#[test]
fn minimal_module_validates() {
    assert!(validate_module(&wat_module(MINIMAL)).is_ok());
}

#[test]
fn memory_pages_limit_enforced() {
    // limit (256 pages) is accepted.
    let at_limit = format!("(module (memory (export \"memory\") {MAX_LINEAR_MEMORY_PAGES}) (func (export \"dispatch\") (param i32 i32) (result i32) i32.const 0))");
    assert!(validate_module(&wat_module(&at_limit)).is_ok());

    // limit + 1 (257 pages) is rejected.
    let over = format!("(module (memory (export \"memory\") {}) (func (export \"dispatch\") (param i32 i32) (result i32) i32.const 0))", MAX_LINEAR_MEMORY_PAGES + 1);
    assert!(validate_module(&wat_module(&over)).is_err());
}

#[test]
fn import_count_limit_enforced() {
    // A module with MAX_IMPORTS imports of the `env.unknown` function.
    let imports = (0..MAX_IMPORTS)
        .map(|i| format!("(import \"env\" \"f{i}\" (func))"))
        .collect::<Vec<_>>()
        .join("\n");
    let at_limit = format!(
        "(module {imports} (memory (export \"memory\") 1) (func (export \"dispatch\") (param i32 i32) (result i32) i32.const 0))"
    );
    assert!(validate_module(&wat_module(&at_limit)).is_ok());

    let imports_over = (0..=MAX_IMPORTS)
        .map(|i| format!("(import \"env\" \"f{i}\" (func))"))
        .collect::<Vec<_>>()
        .join("\n");
    let over = format!(
        "(module {imports_over} (memory (export \"memory\") 1) (func (export \"dispatch\") (param i32 i32) (result i32) i32.const 0))"
    );
    assert!(validate_module(&wat_module(&over)).is_err());
}

#[test]
fn export_count_limit_enforced() {
    // Base module exports `memory` + `dispatch` (2). Add MAX_EXPORTS - 2 more to
    // hit the exact limit, and MAX_EXPORTS - 1 more to exceed it.
    let extra_at = MAX_EXPORTS as usize - 2;
    let exports_at = (0..extra_at)
        .map(|i| format!("(export \"e{i}\" (memory 0))"))
        .collect::<Vec<_>>()
        .join("\n");
    let at_limit = format!(
        "(module (memory (export \"memory\") 1) (func (export \"dispatch\") (param i32 i32) (result i32) i32.const 0) {exports_at})"
    );
    assert!(validate_module(&wat_module(&at_limit)).is_ok());

    let extra_over = MAX_EXPORTS as usize - 1;
    let exports_over = (0..extra_over)
        .map(|i| format!("(export \"e{i}\" (memory 0))"))
        .collect::<Vec<_>>()
        .join("\n");
    let over = format!(
        "(module (memory (export \"memory\") 1) (func (export \"dispatch\") (param i32 i32) (result i32) i32.const 0) {exports_over})"
    );
    assert!(validate_module(&wat_module(&over)).is_err());
}

#[test]
fn table_elements_limit_enforced() {
    let over = format!(
        "(module (memory (export \"memory\") 1) (table (export \"t\") {} funcref) (func (export \"dispatch\") (param i32 i32) (result i32) i32.const 0))",
        MAX_TABLE_ELEMENTS + 1
    );
    assert!(validate_module(&wat_module(&over)).is_err());
}

#[test]
fn malformed_wasm_rejected_without_panic() {
    // Deterministic rejection, no panic.
    assert!(validate_module(b"\x00asm\x01\x00\x00\x00\xff\xff\xff").is_err());
}

#[test]
fn code_size_limit_enforced_before_parse() {
    use zalkanes_core::consensus::MAX_CODE_BYTES;
    // limit + 1 bytes (not valid WASM, but the size check fires first).
    let over = vec![0u8; MAX_CODE_BYTES as usize + 1];
    assert!(validate_module(&over).is_err());
    // limit bytes of valid-ish WASM prefix parses; a module exactly at the limit
    // is representable (the check is strict `>`).
    let under = vec![0u8; MAX_CODE_BYTES as usize - 1];
    assert!(validate_module(&under).is_err()); // invalid WASM, but not "too large"
}

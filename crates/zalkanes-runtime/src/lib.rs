//! # zalkanes-runtime
//!
//! Wasmi 2.0.0 execution profile for Zalkanes v0.
//!
//! **Consensus-critical.** See `docs/wasm-consensus.md` and ADR 0005.
//! Do NOT upgrade Wasmi without opening a protocol-version ADR and
//! regenerating `test-vectors/wasm/fuel-v0.json`.
//!
//! Execution reads through a [`StateStore`] and buffers writes in a local
//! overlay; writes are returned to the caller for atomic commit, and are
//! discarded on trap/fuel exhaustion.

#![forbid(unsafe_code)]

use wasmi::{CompilationMode, Config, Engine, Linker, Module, Store};
use zalkanes_core::{
    consensus::*,
    types::{BlockHeight, ContractId, TxId},
};
use zalkanes_state::StateStore;

/// Result of a single contract call.
#[derive(Debug, Clone)]
pub enum CallResult {
    Success {
        output: Vec<u8>,
        fuel_used: u64,
        /// Storage writes produced by this call (committed on success).
        writes: Vec<StorageWrite>,
    },
    Trap {
        reason: String,
        fuel_used: u64,
    },
    FuelExhausted {
        fuel_used: u64,
    },
    InvalidModule {
        reason: String,
    },
    ContractNotFound,
}

/// A single storage mutation produced by contract execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StorageWrite {
    /// (contract_id, key, value)
    Set(ContractId, Vec<u8>, Vec<u8>),
    /// (contract_id, key)
    Delete(ContractId, Vec<u8>),
}

/// Context provided to a contract during execution.
#[derive(Debug, Clone)]
pub struct CallContext {
    pub contract_id: ContractId,
    pub caller: Option<ContractId>,
    pub txid: TxId,
    pub block_height: BlockHeight,
    pub opcode: u16,
    pub input: Vec<u8>,
    pub fuel_limit: u64,
    pub depth: u32,
}

/// Validate a WASM module against the v0 feature set.
pub fn validate_module(wasm: &[u8]) -> Result<(), String> {
    if wasm.len() as u32 > MAX_CODE_BYTES {
        return Err(format!(
            "module too large: {} > {}",
            wasm.len(),
            MAX_CODE_BYTES
        ));
    }
    let engine = Engine::default();
    Module::new(&engine, wasm).map_err(|e| format!("WASM parse error: {e}"))?;
    Ok(())
}

struct HostState<'a> {
    contract_id: ContractId,
    input: Vec<u8>,
    output: Vec<u8>,
    block_height: BlockHeight,
    /// Write buffer: reads consult this first, then the base store.
    overlay: std::collections::BTreeMap<Vec<u8>, Option<Vec<u8>>>,
    base: &'a dyn StateStore,
}

/// Execute a contract call against a state store, buffering writes.
///
/// On success, the returned [`CallResult::Success`] carries `writes` that the
/// caller MUST commit atomically. On trap/fuel exhaustion, writes are discarded.
pub fn execute(ctx: CallContext, state: &dyn StateStore) -> CallResult {
    if ctx.depth > MAX_CALL_DEPTH {
        return CallResult::Trap {
            reason: format!(
                "call depth {} exceeds MAX_CALL_DEPTH {}",
                ctx.depth, MAX_CALL_DEPTH
            ),
            fuel_used: 0,
        };
    }

    let wasm = match state.get_contract(&ctx.contract_id) {
        Some((_, code)) => code,
        None => return CallResult::ContractNotFound,
    };

    // Build engine with fuel metering (consensus-critical config)
    let mut config = Config::default();
    config.consume_fuel(true);
    config.compilation_mode(CompilationMode::Eager);
    let engine = Engine::new(&config);

    let module = match Module::new(&engine, &wasm) {
        Ok(m) => m,
        Err(e) => {
            return CallResult::InvalidModule {
                reason: e.to_string(),
            }
        }
    };

    let host = HostState {
        contract_id: ctx.contract_id,
        input: ctx.input.clone(),
        output: Vec::new(),
        block_height: ctx.block_height,
        overlay: Default::default(),
        base: state,
    };

    let mut store = Store::new(&engine, host);
    if let Err(e) = store.set_fuel(ctx.fuel_limit) {
        return CallResult::Trap {
            reason: format!("set_fuel failed: {e}"),
            fuel_used: 0,
        };
    }

    let linker = build_linker(&engine);

    let instance = match linker.instantiate_and_start(&mut store, &module) {
        Ok(i) => i,
        Err(e) => {
            let fuel_used = ctx.fuel_limit.saturating_sub(store.get_fuel().unwrap_or(0));
            return CallResult::Trap {
                reason: format!("instantiation error: {e}"),
                fuel_used,
            };
        }
    };

    let dispatch_fn = match instance.get_typed_func::<(i32, i32), i32>(&store, "dispatch") {
        Ok(f) => f,
        Err(_) => {
            return CallResult::Trap {
                reason: "missing 'dispatch' export".to_string(),
                fuel_used: 0,
            };
        }
    };

    match dispatch_fn.call(&mut store, (ctx.opcode as i32, ctx.input.len() as i32)) {
        Ok(0) => {
            let fuel_remaining = store.get_fuel().unwrap_or(0);
            let fuel_used = ctx.fuel_limit.saturating_sub(fuel_remaining);
            let output = store.data().output.clone();

            // Materialize overlay writes.
            let mut writes = Vec::new();
            let contract_id = store.data().contract_id;
            for (key, val) in std::mem::take(&mut store.data_mut().overlay) {
                match val {
                    Some(v) => writes.push(StorageWrite::Set(contract_id, key, v)),
                    None => writes.push(StorageWrite::Delete(contract_id, key)),
                }
            }
            CallResult::Success {
                output,
                fuel_used,
                writes,
            }
        }
        Ok(code) => {
            let fuel_remaining = store.get_fuel().unwrap_or(0);
            let fuel_used = ctx.fuel_limit.saturating_sub(fuel_remaining);
            CallResult::Trap {
                reason: format!("dispatch returned error code {code}"),
                fuel_used,
            }
        }
        Err(e) => {
            let fuel_remaining = store.get_fuel().unwrap_or(0);
            let fuel_used = ctx.fuel_limit.saturating_sub(fuel_remaining);
            let msg = e.to_string();
            if msg.contains("fuel") || msg.contains("OutOfFuel") {
                CallResult::FuelExhausted { fuel_used }
            } else {
                CallResult::Trap {
                    reason: msg,
                    fuel_used,
                }
            }
        }
    }
}

fn build_linker<'a>(engine: &Engine) -> Linker<HostState<'a>> {
    let mut linker = Linker::new(engine);

    // storage_get(key_ptr, key_len, val_ptr) -> i32 (value len, or -1 if missing)
    linker
        .func_wrap(
            "env",
            "storage_get",
            |mut caller: wasmi::Caller<'_, HostState<'a>>,
             key_ptr: i32,
             key_len: i32,
             val_ptr: i32|
             -> i32 {
                let mem = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                    Some(m) => m,
                    None => return -1,
                };
                let key = {
                    let data = mem.data(&caller);
                    let s = key_ptr as usize;
                    let e = s.saturating_add(key_len as usize);
                    if e > data.len() {
                        return -1;
                    }
                    data[s..e].to_vec()
                };
                // Consult overlay first.
                let val = if let Some(v) = caller.data().overlay.get(&key) {
                    v.clone()
                } else {
                    let cid = caller.data().contract_id;
                    caller.data().base.storage_get(&cid, &key)
                };
                match val {
                    None => -1,
                    Some(v) => {
                        let vlen = v.len();
                        let data = mem.data_mut(&mut caller);
                        let dst = val_ptr as usize;
                        if dst + vlen > data.len() {
                            return -1;
                        }
                        data[dst..dst + vlen].copy_from_slice(&v);
                        vlen as i32
                    }
                }
            },
        )
        .ok();

    // storage_set(key_ptr, key_len, val_ptr, val_len) -> i32
    linker
        .func_wrap(
            "env",
            "storage_set",
            |mut caller: wasmi::Caller<'_, HostState<'a>>,
             key_ptr: i32,
             key_len: i32,
             val_ptr: i32,
             val_len: i32|
             -> i32 {
                if key_len as u32 > MAX_STORAGE_KEY_BYTES {
                    return -1;
                }
                if val_len as u32 > MAX_STORAGE_VALUE_BYTES {
                    return -1;
                }
                let mem = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                    Some(m) => m,
                    None => return -1,
                };
                let (key, val) = {
                    let data = mem.data(&caller);
                    let ks = key_ptr as usize;
                    let ke = ks.saturating_add(key_len as usize);
                    let vs = val_ptr as usize;
                    let ve = vs.saturating_add(val_len as usize);
                    if ke > data.len() || ve > data.len() {
                        return -1;
                    }
                    (data[ks..ke].to_vec(), data[vs..ve].to_vec())
                };
                caller.data_mut().overlay.insert(key, Some(val));
                0
            },
        )
        .ok();

    // storage_delete(key_ptr, key_len) -> i32
    linker
        .func_wrap(
            "env",
            "storage_delete",
            |mut caller: wasmi::Caller<'_, HostState<'a>>, key_ptr: i32, key_len: i32| -> i32 {
                let mem = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                    Some(m) => m,
                    None => return -1,
                };
                let key = {
                    let data = mem.data(&caller);
                    let s = key_ptr as usize;
                    let e = s.saturating_add(key_len as usize);
                    if e > data.len() {
                        return -1;
                    }
                    data[s..e].to_vec()
                };
                caller.data_mut().overlay.insert(key, None);
                0
            },
        )
        .ok();

    // output_write(ptr, len) -> i32
    linker
        .func_wrap(
            "env",
            "output_write",
            |mut caller: wasmi::Caller<'_, HostState<'a>>, ptr: i32, len: i32| -> i32 {
                if len as u32 > MAX_RETURN_DATA_BYTES {
                    return -1;
                }
                let mem = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                    Some(m) => m,
                    None => return -1,
                };
                let out = {
                    let data = mem.data(&caller);
                    let s = ptr as usize;
                    let e = s.saturating_add(len as usize);
                    if e > data.len() {
                        return -1;
                    }
                    data[s..e].to_vec()
                };
                caller.data_mut().output = out;
                0
            },
        )
        .ok();

    // context_block_height(out_ptr) -> i32
    linker
        .func_wrap(
            "env",
            "context_block_height",
            |mut caller: wasmi::Caller<'_, HostState<'a>>, out_ptr: i32| -> i32 {
                let height = caller.data().block_height;
                let mem = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                    Some(m) => m,
                    None => return -1,
                };
                let data = mem.data_mut(&mut caller);
                let s = out_ptr as usize;
                if s + 4 > data.len() {
                    return -1;
                }
                data[s..s + 4].copy_from_slice(&height.to_be_bytes());
                0
            },
        )
        .ok();

    // input_read(out_ptr, offset, len) -> i32 (bytes written)
    linker
        .func_wrap(
            "env",
            "input_read",
            |mut caller: wasmi::Caller<'_, HostState<'a>>,
             out_ptr: i32,
             offset: i32,
             len: i32|
             -> i32 {
                let input = caller.data().input.clone();
                let off = offset as usize;
                if off >= input.len() {
                    return 0;
                }
                let slice = &input[off..input.len().min(off + len as usize)];
                let slen = slice.len();
                let mem = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                    Some(m) => m,
                    None => return -1,
                };
                let data = mem.data_mut(&mut caller);
                let s = out_ptr as usize;
                if s + slen > data.len() {
                    return -1;
                }
                data[s..s + slen].copy_from_slice(slice);
                slen as i32
            },
        )
        .ok();

    linker
}

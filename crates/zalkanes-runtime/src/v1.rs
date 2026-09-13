//! Protocol V1 execution engine (ADR-0008).
//!
//! Executes one top-level V1 message: extended host ABI (assets, caller,
//! cross-contract calls, spawn, events) with a frame journal that gives
//! deterministic nested-call revert semantics. The v0 executor
//! (`crate::execute`) is untouched; V1 is a parallel path selected by the
//! indexer for V1 wire messages only.
//!
//! Determinism: same consensus engine configuration as v0 (floats off,
//! fuel metering on, fresh engine + instance per frame), single fuel
//! meter shared across the whole frame stack, BTreeMap-only iteration.

#![allow(clippy::too_many_arguments)]

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use wasmi::{Engine, Linker, Store};
use zalkanes_core::consensus::{
    FUEL_ASSET_OP, FUEL_CONTEXT_OP, FUEL_CONTRACT_CALL_BASE, FUEL_CONTRACT_SPAWN, FUEL_EVENT_BASE,
    FUEL_EVENT_PER_BYTE, MAX_CALL_DEPTH, MAX_EVENTS_PER_CALL, MAX_EVENT_BYTES,
    MAX_LINEAR_MEMORY_PAGES, MAX_RETURN_DATA_BYTES, MAX_STORAGE_KEY_BYTES, MAX_STORAGE_VALUE_BYTES,
    MAX_TABLE_ELEMENTS,
};
use zalkanes_core::types::{v1_spawned_contract_id, CodeHash, ContractId, Network, TxId};

use crate::{consensus_config, parse_module_contained};

/// V1 holder wire form: tag (0 external / 1 contract) ‖ 32-byte id.
pub type HolderBytes = [u8; 33];
/// V1 asset id bytes.
pub type AssetBytes = [u8; 32];

/// The reserved Anonymous external account (ADR-0008 §2).
pub const ANONYMOUS_HOLDER: HolderBytes = [0u8; 33];

// ── deterministic host error codes (returned negated to contracts) ─────────
// Aligned with the DEX error model where semantics overlap.
pub const ERR_NOT_INITIALIZED: i32 = 2;
pub const ERR_ZERO_AMOUNT: i32 = 8;
pub const ERR_INSUFFICIENT_BALANCE: i32 = 10;
pub const ERR_OVERFLOW: i32 = 14;
pub const ERR_INVALID: i32 = 18;
pub const ERR_EXHAUSTED: i32 = 20;

/// Read view the executor runs against: the pre-block store PLUS whatever
/// in-block overlay the indexer maintains (sequential intra-block
/// visibility is the CALLER's responsibility — ADR-0008 §11).
pub trait V1StateView {
    fn contract_code(&self, id: &ContractId) -> Option<(CodeHash, Vec<u8>)>;
    fn code_by_hash(&self, hash: &CodeHash) -> Option<Vec<u8>>;
    fn storage_get(&self, contract: &ContractId, key: &[u8]) -> Option<Vec<u8>>;
    fn ledger_get(&self, holder: &HolderBytes, asset: &AssetBytes) -> Option<u128>;
}

/// One top-level V1 message, fully resolved by the indexer (attachments
/// validated against the wire payload; caller/auth already decided).
#[derive(Debug, Clone)]
pub struct V1CallSpec {
    pub target: ContractId,
    pub opcode: u16,
    pub input: Vec<u8>,
    /// Effective caller holder presented to the contract.
    pub caller: HolderBytes,
    /// Assets attached to the call (validated, non-zero, deduplicated).
    pub attached: Vec<(AssetBytes, u128)>,
    /// External account the attachments are debited from (None = no
    /// attachments allowed).
    pub debit_from: Option<HolderBytes>,
    pub height: u32,
    pub txid: TxId,
    pub network: Network,
    pub fuel_limit: u64,
}

/// Committed effects of a successful V1 message.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct V1Effects {
    /// Final overlay values: Some = upsert, None = delete.
    pub storage: Vec<(ContractId, Vec<u8>, Option<Vec<u8>>)>,
    /// Final ledger values for touched rows (0 = row deleted).
    pub ledger: Vec<(HolderBytes, AssetBytes, u128)>,
    /// Contracts spawned by this message, in spawn order.
    pub spawns: Vec<(ContractId, CodeHash)>,
    /// Events in commit order: (emitting contract, bytes).
    pub events: Vec<(ContractId, Vec<u8>)>,
}

/// Outcome of a V1 message.
#[derive(Debug, Clone)]
pub enum V1Outcome {
    Success {
        output: Vec<u8>,
        fuel_used: u64,
        effects: V1Effects,
    },
    Trap {
        reason: String,
        fuel_used: u64,
    },
    FuelExhausted {
        fuel_used: u64,
    },
    ContractNotFound,
}

// ── frame journal ───────────────────────────────────────────────────────────

#[derive(Default)]
struct Journal {
    storage: BTreeMap<(ContractId, Vec<u8>), Option<Vec<u8>>>,
    ledger: BTreeMap<(HolderBytes, AssetBytes), u128>,
    spawns: Vec<(ContractId, CodeHash)>,
    events: Vec<(ContractId, Vec<u8>)>,
    undo: Vec<JournalUndo>,
}

enum JournalUndo {
    Storage {
        key: (ContractId, Vec<u8>),
        prev: Option<Option<Vec<u8>>>,
    },
    Ledger {
        key: (HolderBytes, AssetBytes),
        prev: Option<u128>,
    },
}

#[derive(Clone, Copy)]
struct Checkpoint {
    undo_len: usize,
    spawns_len: usize,
    events_len: usize,
}

impl Journal {
    fn checkpoint(&self) -> Checkpoint {
        Checkpoint {
            undo_len: self.undo.len(),
            spawns_len: self.spawns.len(),
            events_len: self.events.len(),
        }
    }

    /// Discard everything after `cp` (failed nested frame).
    fn revert_to(&mut self, cp: Checkpoint) {
        while self.undo.len() > cp.undo_len {
            match self.undo.pop().expect("undo entry") {
                JournalUndo::Storage { key, prev } => match prev {
                    Some(entry) => {
                        self.storage.insert(key, entry);
                    }
                    None => {
                        self.storage.remove(&key);
                    }
                },
                JournalUndo::Ledger { key, prev } => match prev {
                    Some(v) => {
                        self.ledger.insert(key, v);
                    }
                    None => {
                        self.ledger.remove(&key);
                    }
                },
            }
        }
        self.spawns.truncate(cp.spawns_len);
        self.events.truncate(cp.events_len);
    }

    fn storage_set(&mut self, contract: ContractId, key: Vec<u8>, value: Option<Vec<u8>>) {
        let map_key = (contract, key);
        let prev = self.storage.get(&map_key).cloned();
        self.undo.push(JournalUndo::Storage {
            key: map_key.clone(),
            prev: match self.storage.contains_key(&map_key) {
                true => Some(prev.expect("present")),
                false => None,
            },
        });
        self.storage.insert(map_key, value);
    }

    fn ledger_set(&mut self, holder: HolderBytes, asset: AssetBytes, value: u128) {
        let key = (holder, asset);
        let prev = self.ledger.get(&key).copied();
        self.undo.push(JournalUndo::Ledger { key, prev });
        self.ledger.insert(key, value);
    }
}

struct Shared<'a> {
    view: &'a dyn V1StateView,
    journal: Journal,
    height: u32,
    txid: TxId,
    network: Network,
    depth: u32,
}

impl Shared<'_> {
    fn storage_read(&self, contract: &ContractId, key: &[u8]) -> Option<Vec<u8>> {
        match self.journal.storage.get(&(*contract, key.to_vec())) {
            Some(entry) => entry.clone(),
            None => self.view.storage_get(contract, key),
        }
    }

    fn ledger_read(&self, holder: &HolderBytes, asset: &AssetBytes) -> u128 {
        match self.journal.ledger.get(&(*holder, *asset)) {
            Some(v) => *v,
            None => self.view.ledger_get(holder, asset).unwrap_or(0),
        }
    }

    fn contract_code(&self, id: &ContractId) -> Option<(CodeHash, Vec<u8>)> {
        if let Some((_, hash)) = self.journal.spawns.iter().find(|(sid, _)| sid == id) {
            let code = self.view.code_by_hash(hash)?;
            return Some((*hash, code));
        }
        self.view.contract_code(id)
    }

    /// Balance-checked ledger transfer inside the journal.
    fn ledger_transfer(
        &mut self,
        from: &HolderBytes,
        to: &HolderBytes,
        asset: &AssetBytes,
        amount: u128,
    ) -> Result<(), i32> {
        if amount == 0 {
            return Err(ERR_ZERO_AMOUNT);
        }
        if *to == ANONYMOUS_HOLDER || to[0] > 1 {
            return Err(ERR_INVALID);
        }
        let from_balance = self.ledger_read(from, asset);
        let remaining = from_balance
            .checked_sub(amount)
            .ok_or(ERR_INSUFFICIENT_BALANCE)?;
        let to_balance = self.ledger_read(to, asset);
        let credited = to_balance.checked_add(amount).ok_or(ERR_OVERFLOW)?;
        self.journal.ledger_set(*from, *asset, remaining);
        self.journal.ledger_set(*to, *asset, credited);
        Ok(())
    }
}

// ── frame host state ────────────────────────────────────────────────────────

struct FrameHost<'a> {
    shared: Rc<RefCell<Shared<'a>>>,
    self_id: ContractId,
    caller: HolderBytes,
    incoming: Vec<(AssetBytes, u128)>,
    input: Vec<u8>,
    output: Vec<u8>,
    limits: wasmi::StoreLimits,
}

fn holder_contract(id: &ContractId) -> HolderBytes {
    let mut out = [0u8; 33];
    out[0] = 1;
    out[1..].copy_from_slice(&id.0);
    out
}

/// Charge a host surcharge against the store's wasmi fuel.
/// Returns false (and zeroes the fuel) when the budget is exhausted.
fn charge<T>(caller: &mut wasmi::Caller<'_, T>, units: u64) -> bool {
    let remaining = caller.get_fuel().unwrap_or(0);
    if remaining < units {
        let _ = caller.set_fuel(0);
        return false;
    }
    let _ = caller.set_fuel(remaining - units);
    true
}

fn read_mem<T>(
    caller: &wasmi::Caller<'_, T>,
    mem: &wasmi::Memory,
    ptr: i32,
    len: usize,
) -> Option<Vec<u8>> {
    let data = mem.data(caller);
    let start = usize::try_from(ptr).ok()?;
    let end = start.checked_add(len)?;
    if end > data.len() {
        return None;
    }
    Some(data[start..end].to_vec())
}

fn write_mem<T>(
    caller: &mut wasmi::Caller<'_, T>,
    mem: &wasmi::Memory,
    ptr: i32,
    bytes: &[u8],
) -> bool {
    let data = mem.data_mut(caller);
    let Ok(start) = usize::try_from(ptr) else {
        return false;
    };
    let Some(end) = start.checked_add(bytes.len()) else {
        return false;
    };
    if end > data.len() {
        return false;
    }
    data[start..end].copy_from_slice(bytes);
    true
}

fn memory_of<T>(caller: &mut wasmi::Caller<'_, T>) -> Option<wasmi::Memory> {
    caller.get_export("memory").and_then(|e| e.into_memory())
}

/// Execute one top-level V1 message.
pub fn execute_v1(view: &dyn V1StateView, spec: V1CallSpec) -> V1Outcome {
    let shared = Rc::new(RefCell::new(Shared {
        view,
        journal: Journal::default(),
        height: spec.height,
        txid: spec.txid,
        network: spec.network,
        depth: 0,
    }));

    // Debit attachments from the external payer into the target's custody
    // (journaled: reverts with everything else on failure).
    if !spec.attached.is_empty() {
        let Some(payer) = spec.debit_from else {
            return V1Outcome::Trap {
                reason: "attachments without an authenticated payer".into(),
                fuel_used: 0,
            };
        };
        let target_holder = holder_contract(&spec.target);
        let mut guard = shared.borrow_mut();
        for (asset, amount) in &spec.attached {
            if let Err(code) = guard.ledger_transfer(&payer, &target_holder, asset, *amount) {
                return V1Outcome::Trap {
                    reason: format!("attachment debit failed with code {code}"),
                    fuel_used: 0,
                };
            }
        }
    }

    let (result, fuel_left) = run_frame(
        &shared,
        spec.target,
        spec.opcode,
        &spec.input,
        spec.caller,
        spec.attached.clone(),
        spec.fuel_limit,
    );
    let fuel_used = spec.fuel_limit.saturating_sub(fuel_left);

    match result {
        FrameResult::Success(output) => {
            let mut shared = Rc::try_unwrap(shared)
                .unwrap_or_else(|_| panic!("frame stack fully unwound"))
                .into_inner();
            let journal = std::mem::take(&mut shared.journal);
            let mut effects = V1Effects::default();
            for ((contract, key), value) in journal.storage {
                effects.storage.push((contract, key, value));
            }
            for ((holder, asset), amount) in journal.ledger {
                effects.ledger.push((holder, asset, amount));
            }
            effects.spawns = journal.spawns;
            effects.events = journal.events;
            V1Outcome::Success {
                output,
                fuel_used,
                effects,
            }
        }
        FrameResult::ErrorCode(code) => V1Outcome::Trap {
            reason: format!("dispatch returned error code {code}"),
            fuel_used,
        },
        FrameResult::Trap(reason) => {
            if reason.contains("fuel") || reason.contains("OutOfFuel") {
                V1Outcome::FuelExhausted { fuel_used }
            } else {
                V1Outcome::Trap { reason, fuel_used }
            }
        }
        FrameResult::NotFound => V1Outcome::ContractNotFound,
    }
}

enum FrameResult {
    Success(Vec<u8>),
    /// Contract's dispatch returned a nonzero (error) code.
    ErrorCode(i32),
    Trap(String),
    NotFound,
}

/// Run one frame (fresh engine + instance, v0 pattern). Returns the result
/// and the remaining fuel.
fn run_frame(
    shared: &Rc<RefCell<Shared<'_>>>,
    target: ContractId,
    opcode: u16,
    input: &[u8],
    caller_holder: HolderBytes,
    incoming: Vec<(AssetBytes, u128)>,
    fuel: u64,
) -> (FrameResult, u64) {
    {
        let mut guard = shared.borrow_mut();
        if guard.depth >= MAX_CALL_DEPTH {
            return (
                FrameResult::Trap(format!("call depth exceeds {MAX_CALL_DEPTH}")),
                fuel,
            );
        }
        guard.depth += 1;
    }
    let outcome = run_frame_inner(shared, target, opcode, input, caller_holder, incoming, fuel);
    shared.borrow_mut().depth -= 1;
    outcome
}

fn run_frame_inner(
    shared: &Rc<RefCell<Shared<'_>>>,
    target: ContractId,
    opcode: u16,
    input: &[u8],
    caller_holder: HolderBytes,
    incoming: Vec<(AssetBytes, u128)>,
    fuel: u64,
) -> (FrameResult, u64) {
    let wasm = {
        let guard = shared.borrow();
        match guard.contract_code(&target) {
            Some((_, code)) => code,
            None => return (FrameResult::NotFound, fuel),
        }
    };

    let engine = Engine::new(&consensus_config());
    let module = match parse_module_contained(&engine, &wasm) {
        Ok(m) => m,
        Err(reason) => return (FrameResult::Trap(format!("invalid module: {reason}")), fuel),
    };

    let host = FrameHost {
        shared: Rc::clone(shared),
        self_id: target,
        caller: caller_holder,
        incoming,
        input: input.to_vec(),
        output: Vec::new(),
        limits: wasmi::StoreLimitsBuilder::new()
            .memory_size(MAX_LINEAR_MEMORY_PAGES as usize * 65536)
            .table_elements(MAX_TABLE_ELEMENTS as usize)
            .memories(1)
            .tables(1)
            .build(),
    };

    let mut store = Store::new(&engine, host);
    store.limiter(|host| &mut host.limits);
    if store.set_fuel(fuel).is_err() {
        return (FrameResult::Trap("set_fuel failed".into()), fuel);
    }

    let linker = build_v1_linker(&engine);
    let instance = match linker.instantiate_and_start(&mut store, &module) {
        Ok(i) => i,
        Err(e) => {
            let left = store.get_fuel().unwrap_or(0);
            return (FrameResult::Trap(format!("instantiation error: {e}")), left);
        }
    };

    let dispatch = match instance.get_typed_func::<(i32, i32), i32>(&store, "dispatch") {
        Ok(f) => f,
        Err(_) => {
            let left = store.get_fuel().unwrap_or(0);
            return (FrameResult::Trap("missing 'dispatch' export".into()), left);
        }
    };

    let input_len = store.data().input.len() as i32;
    match dispatch.call(&mut store, (i32::from(opcode), input_len)) {
        Ok(0) => {
            let left = store.get_fuel().unwrap_or(0);
            let output = std::mem::take(&mut store.data_mut().output);
            (FrameResult::Success(output), left)
        }
        Ok(code) => {
            let left = store.get_fuel().unwrap_or(0);
            (FrameResult::ErrorCode(code), left)
        }
        Err(e) => {
            let left = store.get_fuel().unwrap_or(0);
            (FrameResult::Trap(e.to_string()), left)
        }
    }
}

// ── V1 linker ───────────────────────────────────────────────────────────────

type Ctx<'x, 'a> = wasmi::Caller<'x, FrameHost<'a>>;

fn build_v1_linker<'a>(engine: &Engine) -> Linker<FrameHost<'a>> {
    let mut linker: Linker<FrameHost<'a>> = Linker::new(engine);

    // ── v0 imports (identical observable semantics, journal-backed) ────
    linker
        .func_wrap(
            "env",
            "storage_get",
            |mut caller: Ctx<'_, 'a>, key_ptr: i32, key_len: i32, val_ptr: i32| -> i32 {
                let Some(mem) = memory_of(&mut caller) else {
                    return -1;
                };
                if key_len < 0 {
                    return -1;
                }
                let Some(key) = read_mem(&caller, &mem, key_ptr, key_len as usize) else {
                    return -1;
                };
                let self_id = caller.data().self_id;
                let val = caller.data().shared.borrow().storage_read(&self_id, &key);
                match val {
                    None => -1,
                    Some(v) => {
                        let len = v.len();
                        if !write_mem(&mut caller, &mem, val_ptr, &v) {
                            return -1;
                        }
                        len as i32
                    }
                }
            },
        )
        .ok();

    linker
        .func_wrap(
            "env",
            "storage_set",
            |mut caller: Ctx<'_, 'a>,
             key_ptr: i32,
             key_len: i32,
             val_ptr: i32,
             val_len: i32|
             -> i32 {
                if key_len < 0 || val_len < 0 {
                    return -1;
                }
                if key_len as u32 > MAX_STORAGE_KEY_BYTES
                    || val_len as u32 > MAX_STORAGE_VALUE_BYTES
                {
                    return -1;
                }
                let Some(mem) = memory_of(&mut caller) else {
                    return -1;
                };
                let Some(key) = read_mem(&caller, &mem, key_ptr, key_len as usize) else {
                    return -1;
                };
                let Some(value) = read_mem(&caller, &mem, val_ptr, val_len as usize) else {
                    return -1;
                };
                let self_id = caller.data().self_id;
                caller
                    .data()
                    .shared
                    .borrow_mut()
                    .journal
                    .storage_set(self_id, key, Some(value));
                0
            },
        )
        .ok();

    linker
        .func_wrap(
            "env",
            "storage_delete",
            |mut caller: Ctx<'_, 'a>, key_ptr: i32, key_len: i32| -> i32 {
                if key_len < 0 || key_len as u32 > MAX_STORAGE_KEY_BYTES {
                    return -1;
                }
                let Some(mem) = memory_of(&mut caller) else {
                    return -1;
                };
                let Some(key) = read_mem(&caller, &mem, key_ptr, key_len as usize) else {
                    return -1;
                };
                let self_id = caller.data().self_id;
                caller
                    .data()
                    .shared
                    .borrow_mut()
                    .journal
                    .storage_set(self_id, key, None);
                0
            },
        )
        .ok();

    linker
        .func_wrap(
            "env",
            "context_block_height",
            |mut caller: Ctx<'_, 'a>, out_ptr: i32| -> i32 {
                let Some(mem) = memory_of(&mut caller) else {
                    return -1;
                };
                let height = caller.data().shared.borrow().height;
                if !write_mem(&mut caller, &mem, out_ptr, &height.to_be_bytes()) {
                    return -1;
                }
                0
            },
        )
        .ok();

    linker
        .func_wrap(
            "env",
            "input_read",
            |mut caller: Ctx<'_, 'a>, out_ptr: i32, offset: i32, len: i32| -> i32 {
                let Some(mem) = memory_of(&mut caller) else {
                    return -1;
                };
                if offset < 0 || len < 0 {
                    return -1;
                }
                let input = caller.data().input.clone();
                let start = (offset as usize).min(input.len());
                let end = start.saturating_add(len as usize).min(input.len());
                let chunk = &input[start..end];
                if chunk.is_empty() {
                    return 0;
                }
                if !write_mem(&mut caller, &mem, out_ptr, chunk) {
                    return -1;
                }
                chunk.len() as i32
            },
        )
        .ok();

    linker
        .func_wrap(
            "env",
            "output_write",
            |mut caller: Ctx<'_, 'a>, ptr: i32, len: i32| -> i32 {
                if len < 0 || len as u32 > MAX_RETURN_DATA_BYTES {
                    return -1;
                }
                let Some(mem) = memory_of(&mut caller) else {
                    return -1;
                };
                let Some(bytes) = read_mem(&caller, &mem, ptr, len as usize) else {
                    return -1;
                };
                caller.data_mut().output = bytes;
                0
            },
        )
        .ok();

    // ── V1 extension ────────────────────────────────────────────────────
    linker
        .func_wrap(
            "env",
            "context_self_id",
            |mut caller: Ctx<'_, 'a>, out_ptr: i32| -> i32 {
                if !charge(&mut caller, FUEL_CONTEXT_OP) {
                    return -ERR_EXHAUSTED;
                }
                let Some(mem) = memory_of(&mut caller) else {
                    return -ERR_INVALID;
                };
                let id = caller.data().self_id;
                if !write_mem(&mut caller, &mem, out_ptr, &id.0) {
                    return -ERR_INVALID;
                }
                0
            },
        )
        .ok();

    linker
        .func_wrap(
            "env",
            "context_caller",
            |mut caller: Ctx<'_, 'a>, out_ptr: i32| -> i32 {
                if !charge(&mut caller, FUEL_CONTEXT_OP) {
                    return -ERR_EXHAUSTED;
                }
                let Some(mem) = memory_of(&mut caller) else {
                    return -ERR_INVALID;
                };
                let holder = caller.data().caller;
                if !write_mem(&mut caller, &mem, out_ptr, &holder) {
                    return -ERR_INVALID;
                }
                0
            },
        )
        .ok();

    linker
        .func_wrap(
            "env",
            "incoming_asset_count",
            |mut caller: Ctx<'_, 'a>| -> i32 {
                if !charge(&mut caller, FUEL_CONTEXT_OP) {
                    return -ERR_EXHAUSTED;
                }
                caller.data().incoming.len() as i32
            },
        )
        .ok();

    linker
        .func_wrap(
            "env",
            "incoming_asset_get",
            |mut caller: Ctx<'_, 'a>, index: i32, out_ptr: i32| -> i32 {
                if !charge(&mut caller, FUEL_CONTEXT_OP) {
                    return -ERR_EXHAUSTED;
                }
                let Some(mem) = memory_of(&mut caller) else {
                    return -ERR_INVALID;
                };
                let Ok(idx) = usize::try_from(index) else {
                    return -ERR_INVALID;
                };
                let Some((asset, amount)) = caller.data().incoming.get(idx).copied() else {
                    return -ERR_INVALID;
                };
                let mut buf = [0u8; 48];
                buf[..32].copy_from_slice(&asset);
                buf[32..].copy_from_slice(&amount.to_be_bytes());
                if !write_mem(&mut caller, &mem, out_ptr, &buf) {
                    return -ERR_INVALID;
                }
                0
            },
        )
        .ok();

    linker
        .func_wrap(
            "env",
            "asset_transfer",
            |mut caller: Ctx<'_, 'a>, to_ptr: i32, asset_ptr: i32, amount_ptr: i32| -> i32 {
                if !charge(&mut caller, FUEL_ASSET_OP) {
                    return -ERR_EXHAUSTED;
                }
                let Some(mem) = memory_of(&mut caller) else {
                    return -ERR_INVALID;
                };
                let (Some(to), Some(asset), Some(amount)) = (
                    read_mem(&caller, &mem, to_ptr, 33),
                    read_mem(&caller, &mem, asset_ptr, 32),
                    read_mem(&caller, &mem, amount_ptr, 16),
                ) else {
                    return -ERR_INVALID;
                };
                let to: HolderBytes = to.try_into().expect("33 bytes");
                let asset: AssetBytes = asset.try_into().expect("32 bytes");
                let amount = u128::from_be_bytes(amount.try_into().expect("16 bytes"));
                let from = holder_contract(&caller.data().self_id);
                match caller
                    .data()
                    .shared
                    .borrow_mut()
                    .ledger_transfer(&from, &to, &asset, amount)
                {
                    Ok(()) => 0,
                    Err(code) => -code,
                }
            },
        )
        .ok();

    linker
        .func_wrap(
            "env",
            "asset_mint",
            |mut caller: Ctx<'_, 'a>, to_ptr: i32, amount_ptr: i32| -> i32 {
                if !charge(&mut caller, FUEL_ASSET_OP) {
                    return -ERR_EXHAUSTED;
                }
                let Some(mem) = memory_of(&mut caller) else {
                    return -ERR_INVALID;
                };
                let (Some(to), Some(amount)) = (
                    read_mem(&caller, &mem, to_ptr, 33),
                    read_mem(&caller, &mem, amount_ptr, 16),
                ) else {
                    return -ERR_INVALID;
                };
                let to: HolderBytes = to.try_into().expect("33 bytes");
                let amount = u128::from_be_bytes(amount.try_into().expect("16 bytes"));
                if amount == 0 {
                    return 0;
                }
                if to == ANONYMOUS_HOLDER || to[0] > 1 {
                    return -ERR_INVALID;
                }
                let asset: AssetBytes = caller.data().self_id.0;
                let mut shared = caller.data().shared.borrow_mut();
                let balance = shared.ledger_read(&to, &asset);
                let Some(credited) = balance.checked_add(amount) else {
                    return -ERR_OVERFLOW;
                };
                shared.journal.ledger_set(to, asset, credited);
                0
            },
        )
        .ok();

    linker
        .func_wrap(
            "env",
            "asset_burn",
            |mut caller: Ctx<'_, 'a>, amount_ptr: i32| -> i32 {
                if !charge(&mut caller, FUEL_ASSET_OP) {
                    return -ERR_EXHAUSTED;
                }
                let Some(mem) = memory_of(&mut caller) else {
                    return -ERR_INVALID;
                };
                let Some(amount) = read_mem(&caller, &mem, amount_ptr, 16) else {
                    return -ERR_INVALID;
                };
                let amount = u128::from_be_bytes(amount.try_into().expect("16 bytes"));
                if amount == 0 {
                    return -ERR_ZERO_AMOUNT;
                }
                let self_id = caller.data().self_id;
                let holder = holder_contract(&self_id);
                let asset: AssetBytes = self_id.0;
                let mut shared = caller.data().shared.borrow_mut();
                let balance = shared.ledger_read(&holder, &asset);
                let Some(remaining) = balance.checked_sub(amount) else {
                    return -ERR_INSUFFICIENT_BALANCE;
                };
                shared.journal.ledger_set(holder, asset, remaining);
                0
            },
        )
        .ok();

    linker
        .func_wrap(
            "env",
            "emit_event",
            |mut caller: Ctx<'_, 'a>, ptr: i32, len: i32| -> i32 {
                if len < 0 || len as u32 > MAX_EVENT_BYTES {
                    return -ERR_INVALID;
                }
                if !charge(
                    &mut caller,
                    FUEL_EVENT_BASE + FUEL_EVENT_PER_BYTE * len as u64,
                ) {
                    return -ERR_EXHAUSTED;
                }
                let Some(mem) = memory_of(&mut caller) else {
                    return -ERR_INVALID;
                };
                let Some(bytes) = read_mem(&caller, &mem, ptr, len as usize) else {
                    return -ERR_INVALID;
                };
                let self_id = caller.data().self_id;
                let mut shared = caller.data().shared.borrow_mut();
                if shared.journal.events.len() as u32 >= MAX_EVENTS_PER_CALL {
                    return -ERR_INVALID;
                }
                shared.journal.events.push((self_id, bytes));
                0
            },
        )
        .ok();

    linker
        .func_wrap(
            "env",
            "contract_call",
            |mut caller: Ctx<'_, 'a>,
             target_ptr: i32,
             opcode: i32,
             input_ptr: i32,
             input_len: i32,
             assets_ptr: i32,
             assets_len: i32,
             out_ptr: i32,
             out_cap: i32|
             -> i32 {
                if !charge(&mut caller, FUEL_CONTRACT_CALL_BASE) {
                    return -ERR_EXHAUSTED;
                }
                let Some(mem) = memory_of(&mut caller) else {
                    return -ERR_INVALID;
                };
                if input_len < 0 || assets_len < 0 || out_cap < 0 {
                    return -ERR_INVALID;
                }
                if assets_len % 48 != 0 {
                    return -ERR_INVALID;
                }
                let (Some(target), Some(input), Some(assets_raw)) = (
                    read_mem(&caller, &mem, target_ptr, 32),
                    read_mem(&caller, &mem, input_ptr, input_len as usize),
                    read_mem(&caller, &mem, assets_ptr, assets_len as usize),
                ) else {
                    return -ERR_INVALID;
                };
                let Ok(opcode) = u16::try_from(opcode) else {
                    return -ERR_INVALID;
                };
                let target = ContractId(target.try_into().expect("32 bytes"));
                let mut assets: Vec<(AssetBytes, u128)> = Vec::new();
                for chunk in assets_raw.chunks(48) {
                    let mut asset = [0u8; 32];
                    asset.copy_from_slice(&chunk[..32]);
                    let mut amt = [0u8; 16];
                    amt.copy_from_slice(&chunk[32..]);
                    assets.push((asset, u128::from_be_bytes(amt)));
                }

                let self_id = caller.data().self_id;
                let shared_rc = Rc::clone(&caller.data().shared);

                // Checkpoint + move attached assets into the callee, inside
                // a scoped borrow (never held across the recursion).
                let checkpoint = {
                    let mut shared = shared_rc.borrow_mut();
                    let cp = shared.journal.checkpoint();
                    let from = holder_contract(&self_id);
                    let to = holder_contract(&target);
                    for (asset, amount) in &assets {
                        if let Err(code) = shared.ledger_transfer(&from, &to, asset, *amount) {
                            shared.journal.revert_to(cp);
                            return -code;
                        }
                    }
                    cp
                };

                let fuel = caller.get_fuel().unwrap_or(0);
                let (result, fuel_left) = run_frame(
                    &shared_rc,
                    target,
                    opcode,
                    &input,
                    holder_contract(&self_id),
                    assets,
                    fuel,
                );
                let _ = caller.set_fuel(fuel_left);

                match result {
                    FrameResult::Success(output) => {
                        if output.len() > out_cap as usize {
                            shared_rc.borrow_mut().journal.revert_to(checkpoint);
                            return -ERR_INVALID;
                        }
                        if !write_mem(&mut caller, &mem, out_ptr, &output) {
                            shared_rc.borrow_mut().journal.revert_to(checkpoint);
                            return -ERR_INVALID;
                        }
                        output.len() as i32
                    }
                    FrameResult::ErrorCode(code) => {
                        shared_rc.borrow_mut().journal.revert_to(checkpoint);
                        // Propagate the callee's deterministic error code.
                        -code.clamp(1, i32::MAX)
                    }
                    FrameResult::Trap(reason) => {
                        shared_rc.borrow_mut().journal.revert_to(checkpoint);
                        if reason.contains("fuel") || reason.contains("OutOfFuel") {
                            let _ = caller.set_fuel(0);
                            -ERR_EXHAUSTED
                        } else {
                            -ERR_INVALID
                        }
                    }
                    FrameResult::NotFound => {
                        shared_rc.borrow_mut().journal.revert_to(checkpoint);
                        -ERR_NOT_INITIALIZED
                    }
                }
            },
        )
        .ok();

    linker
        .func_wrap(
            "env",
            "contract_spawn",
            |mut caller: Ctx<'_, 'a>, code_hash_ptr: i32, out_ptr: i32| -> i32 {
                if !charge(&mut caller, FUEL_CONTRACT_SPAWN) {
                    return -ERR_EXHAUSTED;
                }
                let Some(mem) = memory_of(&mut caller) else {
                    return -ERR_INVALID;
                };
                let Some(hash) = read_mem(&caller, &mem, code_hash_ptr, 32) else {
                    return -ERR_INVALID;
                };
                let code_hash = CodeHash(hash.try_into().expect("32 bytes"));
                let self_id = caller.data().self_id;
                let new_id = {
                    let mut shared = caller.data().shared.borrow_mut();
                    if shared.view.code_by_hash(&code_hash).is_none() {
                        return -ERR_INVALID;
                    }
                    let spawn_index = shared.journal.spawns.len();
                    let Ok(spawn_index) = u16::try_from(spawn_index) else {
                        return -ERR_INVALID;
                    };
                    let new_id = v1_spawned_contract_id(
                        shared.network,
                        &shared.txid,
                        spawn_index,
                        &self_id,
                        &code_hash,
                    );
                    if shared.contract_code(&new_id).is_some() {
                        return -ERR_INVALID;
                    }
                    shared.journal.spawns.push((new_id, code_hash));
                    new_id
                };
                if !write_mem(&mut caller, &mem, out_ptr, &new_id.0) {
                    return -ERR_INVALID;
                }
                0
            },
        )
        .ok();

    linker
        .func_wrap(
            "env",
            "fuel_consume",
            |mut caller: Ctx<'_, 'a>, units: i64| -> i32 {
                let Ok(units) = u64::try_from(units) else {
                    return -ERR_INVALID;
                };
                if !charge(&mut caller, units) {
                    return -ERR_EXHAUSTED;
                }
                0
            },
        )
        .ok();

    linker
}

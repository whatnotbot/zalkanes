//! zalkanes-dex-testkit — deterministic runtime for the SUBFROST AMM.
//!
//! Implements the PROPOSED DEX host ABI (`zalkanes_dex_core::host::Host`)
//! as an in-memory chain so the real contract logic (the same code that
//! compiles to wasm32) can be executed and validated end to end:
//!
//! - atomic calls: any failure reverts storage, ledger and events
//! - block model with rollback (reorg), restart (serialize/restore)
//! - canonical BLAKE2b-256 state roots
//! - deterministic fuel metering with injectable limits
//! - fault injection (raw storage writes, fuel caps)
//! - a malicious-token contract for adversarial tests
//!
//! Intra-block semantics NOTE: calls inside one block execute
//! sequentially and each sees the previous call's committed effects.
//! This is the semantics the DEX REQUIRES from the platform and differs
//! from frozen Zalkanes v0 (where every call in a block reads pre-block
//! state). Recorded as upstream blocker UB-5 in `docs/subfrost-amm-v0.md`.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;

use zalkanes_dex_core::error::DexError;
use zalkanes_dex_core::events::DexEvent;
use zalkanes_dex_core::host::Host;
use zalkanes_dex_core::types::{AccountId, AssetId, ContractId, Holder};

const PERSONAL_ROOT: &[u8; 16] = b"ZalkDexRoot0    ";
const PERSONAL_ID: &[u8; 16] = b"ZalkDexSpawnId0 ";
const PERSONAL_TEMPLATE: &[u8; 16] = b"ZalkDexTemplate0";
const PERSONAL_ACCOUNT: &[u8; 16] = b"ZalkDexAccount0 ";

/// Mirror of the platform limits that matter to the DEX.
pub const MAX_CALL_DEPTH: usize = 16;
pub const DEFAULT_FUEL_LIMIT: u64 = 10_000_000;
const MAX_STORAGE_KEY_BYTES: usize = 256;
const MAX_STORAGE_VALUE_BYTES: usize = 65_536;

fn blake32(personal: &[u8; 16], parts: &[&[u8]]) -> [u8; 32] {
    let mut hasher = blake2b_simd::Params::new()
        .hash_length(32)
        .personal(personal)
        .to_state();
    for part in parts {
        hasher.update(&(part.len() as u32).to_be_bytes());
        hasher.update(part);
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(hasher.finalize().as_bytes());
    out
}

/// Behavior modes for the adversarial test token (spec §60).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MaliciousMode {
    /// `get_name` tries to re-enter its caller with `add_liquidity`,
    /// swallows the error, and returns a normal name.
    ReenterAddLiquidity,
    /// `get_name` tries to re-initialize its caller, swallows the error.
    ReenterInitialize,
    /// `get_name` returns a deterministic error (refusing token).
    TrapOnName,
    /// `get_name` returns invalid UTF-8 garbage.
    GarbageName,
    /// `get_name` burns unbounded fuel.
    BurnFuelOnName,
}

impl MaliciousMode {
    fn tag(self) -> u8 {
        match self {
            MaliciousMode::ReenterAddLiquidity => 0,
            MaliciousMode::ReenterInitialize => 1,
            MaliciousMode::TrapOnName => 2,
            MaliciousMode::GarbageName => 3,
            MaliciousMode::BurnFuelOnName => 4,
        }
    }

    fn from_tag(tag: u8) -> Option<Self> {
        Some(match tag {
            0 => MaliciousMode::ReenterAddLiquidity,
            1 => MaliciousMode::ReenterInitialize,
            2 => MaliciousMode::TrapOnName,
            3 => MaliciousMode::GarbageName,
            4 => MaliciousMode::BurnFuelOnName,
            _ => return None,
        })
    }
}

/// The contract templates known to the DEX testkit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ContractKind {
    TestToken,
    SubfrostPool,
    SubfrostFactory,
    MaliciousToken(MaliciousMode),
}

impl ContractKind {
    fn tag(self) -> u8 {
        match self {
            ContractKind::TestToken => 1,
            ContractKind::SubfrostPool => 2,
            ContractKind::SubfrostFactory => 3,
            ContractKind::MaliciousToken(mode) => 100 + mode.tag(),
        }
    }

    fn from_tag(tag: u8) -> Option<Self> {
        Some(match tag {
            1 => ContractKind::TestToken,
            2 => ContractKind::SubfrostPool,
            3 => ContractKind::SubfrostFactory,
            t if t >= 100 => ContractKind::MaliciousToken(MaliciousMode::from_tag(t - 100)?),
            _ => return None,
        })
    }

    /// Deterministic template hash (stand-in for a real WASM code hash).
    #[must_use]
    pub fn template_hash(self) -> [u8; 32] {
        blake32(PERSONAL_TEMPLATE, &[&[self.tag()]])
    }

    fn from_template(hash: &[u8; 32]) -> Option<Self> {
        const ALL: [ContractKind; 8] = [
            ContractKind::TestToken,
            ContractKind::SubfrostPool,
            ContractKind::SubfrostFactory,
            ContractKind::MaliciousToken(MaliciousMode::ReenterAddLiquidity),
            ContractKind::MaliciousToken(MaliciousMode::ReenterInitialize),
            ContractKind::MaliciousToken(MaliciousMode::TrapOnName),
            ContractKind::MaliciousToken(MaliciousMode::GarbageName),
            ContractKind::MaliciousToken(MaliciousMode::BurnFuelOnName),
        ];
        ALL.into_iter().find(|k| &k.template_hash() == hash)
    }
}

/// Deterministic named account fixture.
#[must_use]
pub fn account(name: &str) -> AccountId {
    AccountId(blake32(PERSONAL_ACCOUNT, &[name.as_bytes()]))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ContractInstance {
    kind: ContractKind,
    storage: BTreeMap<Vec<u8>, Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ChainState {
    height: u32,
    contracts: BTreeMap<ContractId, ContractInstance>,
    ledger: BTreeMap<(Holder, AssetId), u128>,
    spawn_nonce: u64,
}

impl ChainState {
    fn genesis() -> Self {
        ChainState {
            height: 0,
            contracts: BTreeMap::new(),
            ledger: BTreeMap::new(),
            spawn_nonce: 0,
        }
    }

    /// Canonical byte serialization; also the state-root preimage.
    fn serialize(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&self.height.to_be_bytes());
        out.extend_from_slice(&self.spawn_nonce.to_be_bytes());
        out.extend_from_slice(&(self.contracts.len() as u32).to_be_bytes());
        for (id, instance) in &self.contracts {
            out.extend_from_slice(&id.0);
            out.push(instance.kind.tag());
            out.extend_from_slice(&(instance.storage.len() as u32).to_be_bytes());
            for (key, value) in &instance.storage {
                out.extend_from_slice(&(key.len() as u32).to_be_bytes());
                out.extend_from_slice(key);
                out.extend_from_slice(&(value.len() as u32).to_be_bytes());
                out.extend_from_slice(value);
            }
        }
        out.extend_from_slice(&(self.ledger.len() as u32).to_be_bytes());
        for ((holder, asset), amount) in &self.ledger {
            out.extend_from_slice(&holder.to_bytes());
            out.extend_from_slice(&asset.0);
            out.extend_from_slice(&amount.to_be_bytes());
        }
        out
    }

    fn deserialize(bytes: &[u8]) -> Option<Self> {
        struct R<'a>(&'a [u8], usize);
        impl<'a> R<'a> {
            fn take(&mut self, n: usize) -> Option<&'a [u8]> {
                let end = self.1.checked_add(n)?;
                if end > self.0.len() {
                    return None;
                }
                let out = &self.0[self.1..end];
                self.1 = end;
                Some(out)
            }
            fn u32(&mut self) -> Option<u32> {
                Some(u32::from_be_bytes(self.take(4)?.try_into().ok()?))
            }
        }
        let mut r = R(bytes, 0);
        let height = r.u32()?;
        let spawn_nonce = u64::from_be_bytes(r.take(8)?.try_into().ok()?);
        let contract_count = r.u32()?;
        let mut contracts = BTreeMap::new();
        for _ in 0..contract_count {
            let id = ContractId(r.take(32)?.try_into().ok()?);
            let kind = ContractKind::from_tag(r.take(1)?[0])?;
            let kv_count = r.u32()?;
            let mut storage = BTreeMap::new();
            for _ in 0..kv_count {
                let klen = r.u32()? as usize;
                let key = r.take(klen)?.to_vec();
                let vlen = r.u32()? as usize;
                let value = r.take(vlen)?.to_vec();
                storage.insert(key, value);
            }
            contracts.insert(id, ContractInstance { kind, storage });
        }
        let ledger_count = r.u32()?;
        let mut ledger = BTreeMap::new();
        for _ in 0..ledger_count {
            let holder = Holder::from_bytes(r.take(33)?).ok()?;
            let asset = AssetId(r.take(32)?.try_into().ok()?);
            let amount = u128::from_be_bytes(r.take(16)?.try_into().ok()?);
            ledger.insert((holder, asset), amount);
        }
        if r.1 != bytes.len() {
            return None;
        }
        Some(ChainState {
            height,
            contracts,
            ledger,
            spawn_nonce,
        })
    }
}

/// Fuel record for one executed call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FuelRecord {
    pub height: u32,
    pub opcode: u16,
    pub fuel_used: u64,
    pub success: bool,
}

struct FuelMeter {
    limit: u64,
    used: u64,
}

impl FuelMeter {
    fn consume(&mut self, units: u64) -> Result<(), DexError> {
        let next = self.used.checked_add(units).ok_or(DexError::OutOfFuel)?;
        if next > self.limit {
            self.used = self.limit;
            return Err(DexError::OutOfFuel);
        }
        self.used = next;
        Ok(())
    }
}

/// The deterministic DEX chain.
pub struct DexChain {
    state: ChainState,
    /// Post-block state per height, for rollback. `history[h]` is the
    /// state after block `h` (index 0 = genesis).
    history: Vec<ChainState>,
    events: Vec<(u32, DexEvent)>,
    fuel_log: Vec<FuelRecord>,
    next_fuel_limit: Option<u64>,
}

impl Default for DexChain {
    fn default() -> Self {
        Self::new()
    }
}

/// One call inside a block.
#[derive(Debug, Clone)]
pub struct Call {
    pub caller: Holder,
    pub target: ContractId,
    pub opcode: u16,
    pub input: Vec<u8>,
    pub assets: Vec<(AssetId, u128)>,
}

impl DexChain {
    #[must_use]
    pub fn new() -> Self {
        let genesis = ChainState::genesis();
        DexChain {
            history: vec![genesis.clone()],
            state: genesis,
            events: Vec::new(),
            fuel_log: Vec::new(),
            next_fuel_limit: None,
        }
    }

    #[must_use]
    pub fn height(&self) -> u32 {
        self.state.height
    }

    /// Canonical state root over contracts, storage and the asset ledger.
    /// Like the platform state root, it deliberately excludes the block
    /// height (and the spawn nonce): an empty block does not change it.
    #[must_use]
    pub fn state_root(&self) -> [u8; 32] {
        blake32(PERSONAL_ROOT, &[&self.state.serialize()[12..]])
    }

    /// State root as of the end of `height` (0 = genesis).
    #[must_use]
    pub fn root_at(&self, height: u32) -> [u8; 32] {
        let state = &self.history[height as usize];
        blake32(PERSONAL_ROOT, &[&state.serialize()[12..]])
    }

    #[must_use]
    pub fn balance(&self, holder: &Holder, asset: &AssetId) -> u128 {
        *self.state.ledger.get(&(*holder, *asset)).unwrap_or(&0)
    }

    #[must_use]
    pub fn events(&self) -> &[(u32, DexEvent)] {
        &self.events
    }

    #[must_use]
    pub fn events_at(&self, height: u32) -> Vec<DexEvent> {
        self.events
            .iter()
            .filter(|(h, _)| *h == height)
            .map(|(_, e)| e.clone())
            .collect()
    }

    #[must_use]
    pub fn fuel_log(&self) -> &[FuelRecord] {
        &self.fuel_log
    }

    #[must_use]
    pub fn last_fuel(&self) -> Option<FuelRecord> {
        self.fuel_log.last().copied()
    }

    /// Cap the fuel limit of the next call only (fault injection).
    pub fn set_next_fuel_limit(&mut self, limit: u64) {
        self.next_fuel_limit = Some(limit);
    }

    /// Deploy a contract instance from a template. Mines one block.
    pub fn deploy(&mut self, kind: ContractKind) -> ContractId {
        let id = derive_contract_id(self.state.height + 1, self.state.spawn_nonce);
        self.state.spawn_nonce += 1;
        self.state.contracts.insert(
            id,
            ContractInstance {
                kind,
                storage: BTreeMap::new(),
            },
        );
        self.seal_block();
        id
    }

    /// Execute one call in its own block.
    pub fn call(
        &mut self,
        caller: Holder,
        target: ContractId,
        opcode: u16,
        input: &[u8],
        assets: &[(AssetId, u128)],
    ) -> Result<Vec<u8>, DexError> {
        let mut results = self.multi_call_block(vec![Call {
            caller,
            target,
            opcode,
            input: input.to_vec(),
            assets: assets.to_vec(),
        }]);
        results.remove(0)
    }

    /// Execute several calls in ONE block, sequentially visible. A failed
    /// call reverts only itself; the block still commits.
    pub fn multi_call_block(&mut self, calls: Vec<Call>) -> Vec<Result<Vec<u8>, DexError>> {
        let executing_height = self.state.height + 1;
        let mut results = Vec::with_capacity(calls.len());
        for call in calls {
            let limit = self.next_fuel_limit.take().unwrap_or(DEFAULT_FUEL_LIMIT);
            let mut fuel = FuelMeter { limit, used: 0 };
            let mut staged_events = Vec::new();
            let snapshot = self.state.clone();
            let mut stack = Vec::new();
            let result = execute(
                &mut self.state,
                &mut staged_events,
                &mut fuel,
                &mut stack,
                executing_height,
                call.caller,
                call.target,
                call.opcode,
                &call.input,
                &call.assets,
            );
            match &result {
                Ok(_) => {
                    for event in staged_events {
                        self.events.push((executing_height, event));
                    }
                }
                Err(_) => {
                    // Full atomic revert of this call.
                    self.state = snapshot;
                }
            }
            self.fuel_log.push(FuelRecord {
                height: executing_height,
                opcode: call.opcode,
                fuel_used: fuel.used,
                success: result.is_ok(),
            });
            results.push(result);
        }
        self.seal_block();
        results
    }

    /// Read-only execution: state mutations and events are discarded.
    pub fn view(&self, target: ContractId, opcode: u16, input: &[u8]) -> Result<Vec<u8>, DexError> {
        let mut scratch = self.state.clone();
        let mut fuel = FuelMeter {
            limit: DEFAULT_FUEL_LIMIT,
            used: 0,
        };
        let mut staged_events = Vec::new();
        let mut stack = Vec::new();
        execute(
            &mut scratch,
            &mut staged_events,
            &mut fuel,
            &mut stack,
            self.state.height,
            Holder::External(account("viewer")),
            target,
            opcode,
            input,
            &[],
        )
    }

    /// Measured view: returns (output, fuel_used).
    pub fn view_with_fuel(
        &self,
        target: ContractId,
        opcode: u16,
        input: &[u8],
    ) -> (Result<Vec<u8>, DexError>, u64) {
        let mut scratch = self.state.clone();
        let mut fuel = FuelMeter {
            limit: DEFAULT_FUEL_LIMIT,
            used: 0,
        };
        let mut staged_events = Vec::new();
        let mut stack = Vec::new();
        let result = execute(
            &mut scratch,
            &mut staged_events,
            &mut fuel,
            &mut stack,
            self.state.height,
            Holder::External(account("viewer")),
            target,
            opcode,
            input,
            &[],
        );
        (result, fuel.used)
    }

    pub fn mine_empty_block(&mut self) {
        self.seal_block();
    }

    /// Roll back to the post-state of `height` (platform reorg analogue).
    pub fn rollback_to(&mut self, height: u32) {
        assert!(
            (height as usize) < self.history.len(),
            "rollback beyond history"
        );
        self.history.truncate(height as usize + 1);
        self.state = self.history[height as usize].clone();
        self.events.retain(|(h, _)| *h <= height);
        self.fuel_log.retain(|r| r.height <= height);
    }

    /// Serialize the full current state (restart simulation).
    #[must_use]
    pub fn serialize_state(&self) -> Vec<u8> {
        self.state.serialize()
    }

    /// Restore a chain from serialized state, as after a process restart.
    /// History before the restored height is not retained (like a node
    /// restarting from its database).
    #[must_use]
    pub fn restore(bytes: &[u8]) -> Option<Self> {
        let state = ChainState::deserialize(bytes)?;
        Some(DexChain {
            history: vec![state.clone()],
            state,
            events: Vec::new(),
            fuel_log: Vec::new(),
            next_fuel_limit: None,
        })
    }

    /// Fault injection: mint balance out of thin air to a holder.
    pub fn faucet(&mut self, holder: Holder, asset: AssetId, amount: u128) {
        let entry = self.state.ledger.entry((holder, asset)).or_insert(0);
        *entry = entry.checked_add(amount).expect("faucet overflow");
        if *entry == 0 {
            self.state.ledger.remove(&(holder, asset));
        }
        self.seal_block();
    }

    /// Unsolicited transfer straight into a contract's custody, bypassing
    /// any contract call (spec §59 donation scenario). Mines one block.
    pub fn donate(
        &mut self,
        from: Holder,
        to_contract: ContractId,
        asset: AssetId,
        amount: u128,
    ) -> Result<(), DexError> {
        ledger_transfer(
            &mut self.state.ledger,
            &from,
            &Holder::Contract(to_contract),
            &asset,
            amount,
        )?;
        self.seal_block();
        Ok(())
    }

    /// Fault injection: raw storage write into a contract (e.g. forcing
    /// the reentrancy lock on) — models mid-execution state.
    pub fn set_storage_raw(&mut self, contract: &ContractId, key: &[u8], value: &[u8]) {
        let instance = self
            .state
            .contracts
            .get_mut(contract)
            .expect("unknown contract");
        instance.storage.insert(key.to_vec(), value.to_vec());
    }

    /// Read raw storage (assertions).
    #[must_use]
    pub fn get_storage_raw(&self, contract: &ContractId, key: &[u8]) -> Option<Vec<u8>> {
        self.state
            .contracts
            .get(contract)?
            .storage
            .get(key)
            .cloned()
    }

    fn seal_block(&mut self) {
        self.state.height += 1;
        self.history.push(self.state.clone());
    }
}

impl zalkanes_dex_sdk::ViewTransport for DexChain {
    fn view(&self, contract: ContractId, opcode: u16, input: &[u8]) -> Result<Vec<u8>, DexError> {
        DexChain::view(self, contract, opcode, input)
    }
}

/// Execute a fully built [`zalkanes_dex_sdk::CallPlan`] on the chain.
impl DexChain {
    pub fn execute_plan(
        &mut self,
        caller: Holder,
        plan: &zalkanes_dex_sdk::CallPlan,
    ) -> Result<Vec<u8>, DexError> {
        self.call(
            caller,
            plan.contract,
            plan.opcode,
            &plan.input,
            &plan.assets,
        )
    }
}

fn derive_contract_id(height: u32, nonce: u64) -> ContractId {
    ContractId(blake32(
        PERSONAL_ID,
        &[&height.to_be_bytes(), &nonce.to_be_bytes()],
    ))
}

fn ledger_transfer(
    ledger: &mut BTreeMap<(Holder, AssetId), u128>,
    from: &Holder,
    to: &Holder,
    asset: &AssetId,
    amount: u128,
) -> Result<(), DexError> {
    if amount == 0 {
        return Err(DexError::ZeroAmount);
    }
    let from_key = (*from, *asset);
    let available = *ledger.get(&from_key).unwrap_or(&0);
    let remaining = available
        .checked_sub(amount)
        .ok_or(DexError::InsufficientLiquidity)?;
    if remaining == 0 {
        ledger.remove(&from_key);
    } else {
        ledger.insert(from_key, remaining);
    }
    let to_entry = ledger.entry((*to, *asset)).or_insert(0);
    *to_entry = to_entry
        .checked_add(amount)
        .ok_or(DexError::ArithmeticOverflow)?;
    Ok(())
}

/// Execute one call frame (used for both top-level and nested calls).
#[allow(clippy::too_many_arguments)]
fn execute(
    state: &mut ChainState,
    events: &mut Vec<DexEvent>,
    fuel: &mut FuelMeter,
    stack: &mut Vec<ContractId>,
    height: u32,
    caller: Holder,
    target: ContractId,
    opcode: u16,
    input: &[u8],
    assets: &[(AssetId, u128)],
) -> Result<Vec<u8>, DexError> {
    if stack.len() >= MAX_CALL_DEPTH {
        return Err(DexError::OutOfFuel);
    }
    let kind = state
        .contracts
        .get(&target)
        .map(|c| c.kind)
        .ok_or(DexError::InvalidArguments)?;

    // Canonicalize + validate incoming assets, then move them into the
    // target's custody. Any later failure reverts at the caller's
    // snapshot, which restores these transfers too.
    let mut incoming: Vec<(AssetId, u128)> = assets.to_vec();
    incoming.sort_by(|a, b| a.0.cmp(&b.0));
    for window in incoming.windows(2) {
        if window[0].0 == window[1].0 {
            return Err(DexError::InvalidIncomingAssets);
        }
    }
    for (asset, amount) in &incoming {
        ledger_transfer(
            &mut state.ledger,
            &caller,
            &Holder::Contract(target),
            asset,
            *amount,
        )?;
    }

    stack.push(target);
    let result = {
        let mut host = TestHost {
            state,
            events,
            fuel,
            stack,
            height,
            self_id: target,
            caller,
            incoming,
        };
        dispatch_kind(kind, &mut host, opcode, input)
    };
    stack.pop();
    result
}

fn dispatch_kind(
    kind: ContractKind,
    host: &mut TestHost<'_>,
    opcode: u16,
    input: &[u8],
) -> Result<Vec<u8>, DexError> {
    match kind {
        ContractKind::TestToken => test_token::dispatch(host, opcode, input),
        ContractKind::SubfrostPool => subfrost_pool::dispatch(host, opcode, input),
        ContractKind::SubfrostFactory => subfrost_factory::dispatch(host, opcode, input),
        ContractKind::MaliciousToken(mode) => malicious_dispatch(mode, host, opcode, input),
    }
}

/// Adversarial token used only in tests (spec §60).
fn malicious_dispatch(
    mode: MaliciousMode,
    host: &mut TestHost<'_>,
    opcode: u16,
    input: &[u8],
) -> Result<Vec<u8>, DexError> {
    use zalkanes_dex_core::encode::{pool_op, token_op};
    match opcode {
        token_op::GET_NAME => match mode {
            MaliciousMode::TrapOnName => Err(DexError::Unsupported),
            MaliciousMode::GarbageName => Ok(vec![0xff, 0xfe, 0x00, 0x80]),
            MaliciousMode::BurnFuelOnName => loop {
                host.consume_fuel(100_000)?;
            },
            MaliciousMode::ReenterAddLiquidity | MaliciousMode::ReenterInitialize => {
                let pool = match host.caller() {
                    Holder::Contract(id) => id,
                    Holder::External(_) => return Ok(b"EVIL".to_vec()),
                };
                let (op, args) = if mode == MaliciousMode::ReenterAddLiquidity {
                    (
                        pool_op::ADD_LIQUIDITY,
                        zalkanes_dex_core::encode::pool_add_liquidity_args(0, 0),
                    )
                } else {
                    (
                        pool_op::INITIALIZE,
                        zalkanes_dex_core::encode::pool_initialize_args(
                            &AssetId([0xaa; 32]),
                            &AssetId([0xbb; 32]),
                            &Holder::External(account("attacker")),
                        ),
                    )
                };
                // The reentrancy attempt MUST fail; record the outcome in
                // storage so tests can assert it, then behave normally.
                let attack = host.call(&pool, op, &args, &[]);
                let marker: &[u8] = if attack.is_err() { b"blocked" } else { b"HIT" };
                host.storage_set(b"attack", marker)?;
                Ok(b"EVIL".to_vec())
            }
        },
        // Behave like an uninitialized token elsewhere.
        _ => test_token::dispatch(host, opcode, input),
    }
}

struct TestHost<'a> {
    state: &'a mut ChainState,
    events: &'a mut Vec<DexEvent>,
    fuel: &'a mut FuelMeter,
    stack: &'a mut Vec<ContractId>,
    height: u32,
    self_id: ContractId,
    caller: Holder,
    incoming: Vec<(AssetId, u128)>,
}

impl Host for TestHost<'_> {
    fn self_id(&self) -> ContractId {
        self.self_id
    }

    fn caller(&self) -> Holder {
        self.caller
    }

    fn block_height(&self) -> u32 {
        self.height
    }

    fn incoming_assets(&self) -> Vec<(AssetId, u128)> {
        self.incoming.clone()
    }

    fn storage_get(&self, key: &[u8]) -> Option<Vec<u8>> {
        self.state
            .contracts
            .get(&self.self_id)?
            .storage
            .get(key)
            .cloned()
    }

    fn storage_set(&mut self, key: &[u8], value: &[u8]) -> Result<(), DexError> {
        if key.len() > MAX_STORAGE_KEY_BYTES || value.len() > MAX_STORAGE_VALUE_BYTES {
            return Err(DexError::InvalidArguments);
        }
        self.fuel
            .consume(40 + (key.len() as u64 + value.len() as u64) / 8)?;
        self.state
            .contracts
            .get_mut(&self.self_id)
            .ok_or(DexError::InvalidArguments)?
            .storage
            .insert(key.to_vec(), value.to_vec());
        Ok(())
    }

    fn storage_delete(&mut self, key: &[u8]) -> Result<(), DexError> {
        self.fuel.consume(25)?;
        self.state
            .contracts
            .get_mut(&self.self_id)
            .ok_or(DexError::InvalidArguments)?
            .storage
            .remove(key);
        Ok(())
    }

    fn transfer_out(&mut self, to: &Holder, asset: &AssetId, amount: u128) -> Result<(), DexError> {
        self.fuel.consume(60)?;
        ledger_transfer(
            &mut self.state.ledger,
            &Holder::Contract(self.self_id),
            to,
            asset,
            amount,
        )
    }

    fn mint_own_asset(&mut self, to: &Holder, amount: u128) -> Result<(), DexError> {
        self.fuel.consume(60)?;
        if amount == 0 {
            return Ok(());
        }
        let asset = AssetId::of_contract(&self.self_id);
        let entry = self.state.ledger.entry((*to, asset)).or_insert(0);
        *entry = entry
            .checked_add(amount)
            .ok_or(DexError::ArithmeticOverflow)?;
        Ok(())
    }

    fn burn_own_asset(&mut self, amount: u128) -> Result<(), DexError> {
        self.fuel.consume(60)?;
        if amount == 0 {
            return Err(DexError::ZeroAmount);
        }
        let asset = AssetId::of_contract(&self.self_id);
        let key = (Holder::Contract(self.self_id), asset);
        let available = *self.state.ledger.get(&key).unwrap_or(&0);
        let remaining = available
            .checked_sub(amount)
            .ok_or(DexError::InsufficientLp)?;
        if remaining == 0 {
            self.state.ledger.remove(&key);
        } else {
            self.state.ledger.insert(key, remaining);
        }
        Ok(())
    }

    fn emit_event(&mut self, event: &DexEvent) {
        self.events.push(event.clone());
    }

    fn call(
        &mut self,
        target: &ContractId,
        opcode: u16,
        input: &[u8],
        assets: &[(AssetId, u128)],
    ) -> Result<Vec<u8>, DexError> {
        self.fuel.consume(200)?;
        // Nested atomicity: snapshot state + staged events; a failed
        // nested call reverts its own effects only.
        let snapshot = self.state.clone();
        let events_len = self.events.len();
        let result = execute(
            self.state,
            self.events,
            self.fuel,
            self.stack,
            self.height,
            Holder::Contract(self.self_id),
            *target,
            opcode,
            input,
            assets,
        );
        if result.is_err() {
            *self.state = snapshot;
            self.events.truncate(events_len);
        }
        result
    }

    fn spawn(&mut self, template_code_hash: &[u8; 32]) -> Result<ContractId, DexError> {
        self.fuel.consume(500)?;
        let kind =
            ContractKind::from_template(template_code_hash).ok_or(DexError::InvalidArguments)?;
        let id = derive_contract_id(self.height, self.state.spawn_nonce);
        self.state.spawn_nonce += 1;
        if self.state.contracts.contains_key(&id) {
            return Err(DexError::InvalidArguments);
        }
        self.state.contracts.insert(
            id,
            ContractInstance {
                kind,
                storage: BTreeMap::new(),
            },
        );
        Ok(id)
    }

    fn consume_fuel(&mut self, units: u64) -> Result<(), DexError> {
        self.fuel.consume(units)
    }
}

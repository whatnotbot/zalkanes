//! Shared deterministic fixtures for the DEX runtime test suites.

// Each test binary uses a different subset of this shared fixture.
#![allow(dead_code)]

use zalkanes_dex_core::encode::{
    factory_create_pool_args, factory_initialize_args, factory_op, token_initialize_args,
    token_mint_args, token_op,
};
use zalkanes_dex_core::error::DexError;
use zalkanes_dex_core::types::{AssetId, ContractId, Holder};
use zalkanes_dex_testkit::{account, Call, ContractKind, DexChain};

pub const ALICE_ZA: u128 = 1_000_000_000_000;
pub const ALICE_ZB: u128 = 1_000_000_000_000;
pub const BOB_ZA: u128 = 50_000_000;
pub const INIT_A: u128 = 5_000_000;
pub const INIT_B: u128 = 20_000_000;

pub struct Fixture {
    pub chain: DexChain,
    pub alice: Holder,
    pub bob: Holder,
    pub token_a: ContractId,
    pub token_b: ContractId,
    pub za: AssetId,
    pub zb: AssetId,
    pub factory: ContractId,
}

impl Fixture {
    /// Tokens deployed + initialized + funded; factory initialized;
    /// no pool yet.
    pub fn new() -> Self {
        let mut chain = DexChain::new();
        let alice = Holder::External(account("alice"));
        let bob = Holder::External(account("bob"));

        let token_a = chain.deploy(ContractKind::TestToken);
        let token_b = chain.deploy(ContractKind::TestToken);
        let za = AssetId::of_contract(&token_a);
        let zb = AssetId::of_contract(&token_b);

        chain
            .call(
                alice,
                token_a,
                token_op::INITIALIZE,
                &token_initialize_args("Zalkanes Test Asset A"),
                &[],
            )
            .expect("init token A");
        chain
            .call(
                alice,
                token_b,
                token_op::INITIALIZE,
                &token_initialize_args("Zalkanes Test Asset B"),
                &[],
            )
            .expect("init token B");
        chain
            .call(
                alice,
                token_a,
                token_op::MINT_FOR_TEST,
                &token_mint_args(&alice, ALICE_ZA),
                &[],
            )
            .expect("mint ZA to alice");
        chain
            .call(
                alice,
                token_b,
                token_op::MINT_FOR_TEST,
                &token_mint_args(&alice, ALICE_ZB),
                &[],
            )
            .expect("mint ZB to alice");
        chain
            .call(
                alice,
                token_a,
                token_op::MINT_FOR_TEST,
                &token_mint_args(&bob, BOB_ZA),
                &[],
            )
            .expect("mint ZA to bob");

        let factory = chain.deploy(ContractKind::SubfrostFactory);
        chain
            .call(
                alice,
                factory,
                factory_op::INITIALIZE,
                &factory_initialize_args(&ContractKind::SubfrostPool.template_hash()),
                &[],
            )
            .expect("init factory");

        Fixture {
            chain,
            alice,
            bob,
            token_a,
            token_b,
            za,
            zb,
            factory,
        }
    }

    /// Fixture plus the canonical ZA/ZB pool with INIT_A / INIT_B.
    pub fn with_pool() -> (Self, ContractId) {
        let mut fx = Self::new();
        let pool = fx.create_pool(INIT_A, INIT_B).expect("create pool");
        (fx, pool)
    }

    pub fn create_pool(&mut self, amount_a: u128, amount_b: u128) -> Result<ContractId, DexError> {
        let out = self.chain.call(
            self.alice,
            self.factory,
            factory_op::CREATE_POOL,
            &factory_create_pool_args(&self.za, &self.zb),
            &[(self.za, amount_a), (self.zb, amount_b)],
        )?;
        assert_eq!(out.len(), 32, "create_pool returns a pool id");
        let mut id = [0u8; 32];
        id.copy_from_slice(&out);
        Ok(ContractId(id))
    }

    /// Canonical (token0, token1) with the matching (reserve0, reserve1)
    /// interpretation of (amount_a, amount_b).
    pub fn canonical_amounts(
        &self,
        amount_a: u128,
        amount_b: u128,
    ) -> (AssetId, AssetId, u128, u128) {
        if self.za < self.zb {
            (self.za, self.zb, amount_a, amount_b)
        } else {
            (self.zb, self.za, amount_b, amount_a)
        }
    }

    pub fn balance(&self, holder: &Holder, asset: &AssetId) -> u128 {
        self.chain.balance(holder, asset)
    }

    pub fn lp_asset(pool: &ContractId) -> AssetId {
        AssetId::of_contract(pool)
    }

    /// Expiry height that is valid for exactly the next call.
    pub fn next_call_height(&self) -> u32 {
        self.chain.height() + 1
    }

    pub fn simple_call(
        &self,
        caller: Holder,
        target: ContractId,
        opcode: u16,
        input: Vec<u8>,
        assets: Vec<(AssetId, u128)>,
    ) -> Call {
        let _ = self;
        Call {
            caller,
            target,
            opcode,
            input,
            assets,
        }
    }
}

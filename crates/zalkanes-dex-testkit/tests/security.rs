//! §58/§59/§60 — adversarial behavior: malicious tokens, unsolicited
//! transfers (donations), and fuzz-style robustness of every decoder and
//! dispatch surface (deterministic PRNG, no external fuzzer needed).

mod common;

use common::{Fixture, INIT_A, INIT_B};
use zalkanes_dex_core::encode::{
    decode_pool_details, factory_create_pool_args, factory_op, pool_op, pool_quote_args,
    pool_swap_args,
};
use zalkanes_dex_core::error::DexError;
use zalkanes_dex_core::events::DexEvent;
use zalkanes_dex_core::types::{AssetId, ContractId, Holder};
use zalkanes_dex_testkit::{account, ContractKind, DexChain, MaliciousMode};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn bytes(&mut self, max_len: usize) -> Vec<u8> {
        let len = (self.next() as usize) % (max_len + 1);
        (0..len).map(|_| (self.next() & 0xff) as u8).collect()
    }
}

/// Build a pool whose token B is a malicious contract.
fn pool_with_malicious(mode: MaliciousMode) -> (DexChain, Holder, ContractId, AssetId, AssetId) {
    let mut chain = DexChain::new();
    let alice = Holder::External(account("alice"));
    let honest = chain.deploy(ContractKind::TestToken);
    let evil = chain.deploy(ContractKind::MaliciousToken(mode));
    let (ha, ea) = (AssetId::of_contract(&honest), AssetId::of_contract(&evil));
    chain
        .call(
            alice,
            honest,
            zalkanes_dex_core::encode::token_op::INITIALIZE,
            &zalkanes_dex_core::encode::token_initialize_args("Zalkanes Test Asset A"),
            &[],
        )
        .expect("init honest");
    chain.faucet(alice, ha, 100_000_000);
    chain.faucet(alice, ea, 100_000_000);
    let factory = chain.deploy(ContractKind::SubfrostFactory);
    chain
        .call(
            alice,
            factory,
            factory_op::INITIALIZE,
            &zalkanes_dex_core::encode::factory_initialize_args(
                &ContractKind::SubfrostPool.template_hash(),
            ),
            &[],
        )
        .expect("factory init");
    let out = chain
        .call(
            alice,
            factory,
            factory_op::CREATE_POOL,
            &factory_create_pool_args(&ha, &ea),
            &[(ha, INIT_A), (ea, INIT_B)],
        )
        .expect("create with malicious token");
    let mut id = [0u8; 32];
    id.copy_from_slice(&out);
    (chain, alice, ContractId(id), ha, ea)
}

#[test]
fn malicious_reentrant_token_cannot_corrupt_init() {
    for mode in [
        MaliciousMode::ReenterAddLiquidity,
        MaliciousMode::ReenterInitialize,
    ] {
        let (chain, alice, pool, ha, ea) = pool_with_malicious(mode);
        // The attack marker must show the reentrancy was blocked.
        let evil_contract = ContractId(ea.0);
        assert_eq!(
            chain.get_storage_raw(&evil_contract, b"attack"),
            Some(b"blocked".to_vec()),
            "reentrancy attempt must be rejected ({mode:?})"
        );
        // Pool state is fully consistent.
        let details =
            decode_pool_details(&chain.view(pool, pool_op::POOL_DETAILS, &[]).unwrap()).unwrap();
        let (r0, r1) = if ha < ea {
            (INIT_A, INIT_B)
        } else {
            (INIT_B, INIT_A)
        };
        assert_eq!((details.reserve0, details.reserve1), (r0, r1));
        assert_eq!(details.total_lp_supply, 10_000_000); // isqrt(INIT_A*INIT_B)
                                                         // LP conservation: alice holds supply - locked; nobody else holds LP.
        let lp = AssetId::of_contract(&pool);
        assert_eq!(chain.balance(&alice, &lp), 10_000_000 - 1_000);
        assert_eq!(chain.balance(&Holder::Contract(evil_contract), &lp), 0);
    }
}

#[test]
fn malicious_trap_and_garbage_names_fall_back() {
    for mode in [MaliciousMode::TrapOnName, MaliciousMode::GarbageName] {
        let (chain, _alice, pool, ..) = pool_with_malicious(mode);
        let name = chain.view(pool, pool_op::GET_NAME, &[]).expect("name");
        let name = String::from_utf8(name).expect("pool name stays valid UTF-8");
        assert!(name.ends_with(" LP"), "fallback name for {mode:?}: {name}");
    }
}

#[test]
fn malicious_fuel_burner_fails_creation_atomically() {
    // Fuel is a per-call-chain budget (platform semantics): a token that
    // burns unbounded fuel during name resolution kills the whole
    // creation with OutOfFuel — and the failure must be fully atomic.
    let mut chain = DexChain::new();
    let alice = Holder::External(account("alice"));
    let honest = chain.deploy(ContractKind::TestToken);
    let evil = chain.deploy(ContractKind::MaliciousToken(MaliciousMode::BurnFuelOnName));
    let (ha, ea) = (AssetId::of_contract(&honest), AssetId::of_contract(&evil));
    chain.faucet(alice, ha, 100_000_000);
    chain.faucet(alice, ea, 100_000_000);
    let factory = chain.deploy(ContractKind::SubfrostFactory);
    chain
        .call(
            alice,
            factory,
            factory_op::INITIALIZE,
            &zalkanes_dex_core::encode::factory_initialize_args(
                &ContractKind::SubfrostPool.template_hash(),
            ),
            &[],
        )
        .expect("factory init");
    let root_before = chain.state_root();
    let err = chain
        .call(
            alice,
            factory,
            factory_op::CREATE_POOL,
            &factory_create_pool_args(&ha, &ea),
            &[(ha, INIT_A), (ea, INIT_B)],
        )
        .expect_err("fuel burner");
    assert_eq!(err, DexError::OutOfFuel);
    let h = chain.height();
    assert_eq!(chain.root_at(h), root_before, "atomic revert");
    let count = chain.view(factory, factory_op::POOL_COUNT, &[]).unwrap();
    assert_eq!(count, 0u128.to_be_bytes().to_vec(), "no pool registered");
}

#[test]
fn donation_does_not_corrupt_pricing() {
    // §59: reserves are ACCOUNTING state. Unsolicited transfers into the
    // pool's custody must not change reserves, quotes or LP redemption.
    let (mut fx, pool) = Fixture::with_pool();
    let quote_before = fx
        .chain
        .view(
            pool,
            pool_op::QUOTE_EXACT_IN,
            &pool_quote_args(&fx.za, 100_000),
        )
        .expect("quote");
    let details_before =
        decode_pool_details(&fx.chain.view(pool, pool_op::POOL_DETAILS, &[]).unwrap()).unwrap();

    fx.chain
        .donate(fx.alice, pool, fx.za, 3_333_333)
        .expect("donation");

    let quote_after = fx
        .chain
        .view(
            pool,
            pool_op::QUOTE_EXACT_IN,
            &pool_quote_args(&fx.za, 100_000),
        )
        .expect("quote after");
    let details_after =
        decode_pool_details(&fx.chain.view(pool, pool_op::POOL_DETAILS, &[]).unwrap()).unwrap();
    assert_eq!(quote_before, quote_after, "donation must not move price");
    assert_eq!(
        (details_before.reserve0, details_before.reserve1),
        (details_after.reserve0, details_after.reserve1)
    );
    // Physical custody exceeds accounting; the surplus is inert.
    let (t0, ..) = fx.canonical_amounts(INIT_A, INIT_B);
    let custody0 = fx.balance(&Holder::Contract(pool), &t0);
    let accounted0 = details_after.reserve0 + details_after.protocol_fees0;
    if t0 == fx.za {
        assert_eq!(custody0, accounted0 + 3_333_333);
    }
    // Swaps still work and use accounting reserves.
    fx.chain
        .call(
            fx.alice,
            pool,
            pool_op::SWAP_EXACT_IN,
            &pool_swap_args(0, 0),
            &[(fx.za, 100_000)],
        )
        .expect("swap after donation");
}

#[test]
fn fuzz_style_dispatch_robustness() {
    // §30 analogue (deterministic): random opcodes + random calldata +
    // random assets against pool, factory and token. Requirements:
    // deterministic Result, no panic, atomic failure.
    let (mut fx, pool) = Fixture::with_pool();
    let mut rng = Rng(0xdead_beef_cafe_f00d);
    let targets = [pool, fx.factory, fx.token_a];
    for i in 0..400 {
        let target = targets[(rng.next() as usize) % targets.len()];
        let opcode = (rng.next() & 0xffff) as u16;
        let input = rng.bytes(96);
        let attach_za = rng.next() % 3 == 0;
        let assets: Vec<(AssetId, u128)> = if attach_za {
            vec![(fx.za, (rng.next() % 1_000) as u128 + 1)]
        } else {
            vec![]
        };
        let root_before = fx.chain.state_root();
        let result = fx.chain.call(fx.alice, target, opcode, &input, &assets);
        // Determinism: the identical call on the identical state gives
        // the identical result (checked on a restored clone).
        let clone = DexChain::restore(&fx.chain.serialize_state()).expect("clone");
        let mut clone = clone;
        let replay = clone.call(fx.alice, target, opcode, &input, &assets);
        assert_eq!(result, replay, "iteration {i} nondeterministic");
        if result.is_err() {
            let h = fx.chain.height();
            assert_eq!(fx.chain.root_at(h), root_before, "iteration {i} not atomic");
        }
    }
}

#[test]
fn fuzz_style_decoder_robustness() {
    // fuzz_view_decode / fuzz_event_decode analogue: decoders never
    // panic and never accept trailing garbage.
    let mut rng = Rng(0x0dd_ba11);
    for _ in 0..2_000 {
        let bytes = rng.bytes(300);
        let _ = zalkanes_dex_core::encode::decode_pool_details(&bytes);
        let _ = zalkanes_dex_core::encode::decode_swap_quote(&bytes);
        let _ = zalkanes_dex_core::encode::decode_reserves(&bytes);
        let _ = zalkanes_dex_core::encode::decode_pool_list(&bytes);
        let _ = DexEvent::decode(&bytes);
        let _ = zalkanes_dex_core::types::Holder::from_bytes(&bytes);
    }
    // Round-trips stay exact.
    let event = DexEvent::Swap {
        pool: ContractId([9; 32]),
        token_in: AssetId([1; 32]),
        token_out: AssetId([2; 32]),
        amount_in: 12345,
        amount_out: 999,
        total_fee: 123,
        lp_fee: 98,
        protocol_fee: 25,
    };
    assert_eq!(DexEvent::decode(&event.encode()).unwrap(), event);
    // Trailing garbage must be rejected.
    let mut bytes = event.encode();
    bytes.push(0);
    assert_eq!(DexEvent::decode(&bytes), Err(DexError::InvalidArguments));
}

#[test]
fn events_only_for_committed_transitions() {
    // §24: no event may survive from a reverted call.
    let (mut fx, pool) = Fixture::with_pool();
    let events_before = fx.chain.events().len();
    let _ = fx
        .chain
        .call(
            fx.alice,
            pool,
            pool_op::SWAP_EXACT_IN,
            &pool_swap_args(u128::MAX, 0),
            &[(fx.za, 100_000)],
        )
        .expect_err("slippage revert");
    assert_eq!(fx.chain.events().len(), events_before, "no event leaked");
    fx.chain
        .call(
            fx.alice,
            pool,
            pool_op::SWAP_EXACT_IN,
            &pool_swap_args(0, 0),
            &[(fx.za, 100_000)],
        )
        .expect("swap");
    assert_eq!(fx.chain.events().len(), events_before + 1);
}

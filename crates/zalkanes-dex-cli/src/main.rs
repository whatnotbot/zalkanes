//! zalkanes-dex — SUBFROST AMM CLI (v0).
//!
//! Every transaction-producing command is DRY-RUN ONLY in v0: it builds
//! and prints the exact `CallPlan` (contract, opcode, calldata, assets)
//! without broadcasting anything, because the frozen Zalkanes v0 host ABI
//! cannot yet execute DEX contracts (see docs/subfrost-amm-v0.md,
//! "Upstream blockers"). `--broadcast` therefore always fails with
//! BLOCKED-UPSTREAM — and is REFUSED outright on mainnet while
//! `MAINNET_ACTIVATION_HEIGHT` is `None`.
//!
//! Commands:
//!   zalkanes-dex pools --demo
//!   zalkanes-dex pool <POOL_HEX> --demo
//!   zalkanes-dex quote <POOL_HEX> <TOKEN_IN_HEX> <AMOUNT> --demo
//!   zalkanes-dex create-pool <FACTORY> <TOKEN_A> <TOKEN_B> <AMT_A> <AMT_B>
//!   zalkanes-dex add-liquidity <POOL> <T0> <T1> <D0> <D1> <MIN_LP> <EXPIRY>
//!   zalkanes-dex remove-liquidity <POOL> <LP> <MIN0> <MIN1> <EXPIRY>
//!   zalkanes-dex swap <POOL> <TOKEN_IN> <AMOUNT> <MIN_OUT> <EXPIRY>
//!
//! Network selection: --network <regtest|testnet|mainnet> or
//! $ZALKANES_NETWORK (default regtest), matching the platform CLI.

#![forbid(unsafe_code)]

use zalkanes_core::consensus::MAINNET_ACTIVATION_HEIGHT;
use zalkanes_dex_core::types::{AssetId, ContractId};
use zalkanes_dex_sdk::{
    build_add_liquidity, build_create_pool, build_remove_liquidity, build_swap_exact_in,
    AddLiquidityIntent, CallPlan, CreatePoolIntent, DexView, RemoveLiquidityIntent, SwapIntent,
};

const TX_COMMANDS: [&str; 4] = ["create-pool", "add-liquidity", "remove-liquidity", "swap"];

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    match run(&arg_refs, std::env::var("ZALKANES_NETWORK").ok().as_deref()) {
        Ok(output) => println!("{output}"),
        Err(err) => {
            eprintln!("error: {err}");
            std::process::exit(1);
        }
    }
}

fn parse_id(hex_str: &str) -> Result<[u8; 32], String> {
    let bytes = hex::decode(hex_str).map_err(|_| format!("invalid hex: {hex_str}"))?;
    let arr: [u8; 32] = bytes
        .try_into()
        .map_err(|_| format!("expected 32 bytes: {hex_str}"))?;
    Ok(arr)
}

fn parse_amount(s: &str) -> Result<u128, String> {
    s.replace('_', "")
        .parse::<u128>()
        .map_err(|_| format!("invalid amount: {s}"))
}

fn parse_height(s: &str) -> Result<u32, String> {
    s.parse::<u32>().map_err(|_| format!("invalid height: {s}"))
}

fn render_plan(plan: &CallPlan) -> String {
    let mut out = String::new();
    out.push_str("DRY RUN — no transaction was created or broadcast\n");
    out.push_str(&format!("contract: {}\n", plan.contract));
    out.push_str(&format!("opcode:   {}\n", plan.opcode));
    out.push_str(&format!("calldata: {}\n", hex::encode(&plan.input)));
    for (asset, amount) in &plan.assets {
        out.push_str(&format!("attach:   {amount} of {asset}\n"));
    }
    out.push_str(
        "broadcast: unavailable (BLOCKED-UPSTREAM: frozen Zalkanes v0 host ABI \
         has no asset/call/spawn primitives; see docs/subfrost-amm-v0.md)",
    );
    out
}

/// Pure command core; unit-testable.
fn run(args: &[&str], env_network: Option<&str>) -> Result<String, String> {
    let mut network = env_network.unwrap_or("regtest").to_lowercase();
    let mut broadcast = false;
    let mut demo = false;
    let mut positional: Vec<&str> = Vec::new();
    let mut iter = args.iter().peekable();
    while let Some(arg) = iter.next() {
        match *arg {
            "--network" => {
                network = iter
                    .next()
                    .ok_or("--network requires a value")?
                    .to_lowercase();
            }
            "--broadcast" => broadcast = true,
            "--dry-run" => broadcast = false,
            "--demo" => demo = true,
            other => positional.push(other),
        }
    }
    let network = match network.as_str() {
        "mainnet" | "main" => "mainnet",
        "testnet" | "test" => "testnet",
        "regtest" => "regtest",
        other => return Err(format!("unknown network: {other}")),
    };
    let command = *positional.first().ok_or("no command; see --help in docs")?;

    // §49 hard guard: DEX operations on mainnet are refused outright
    // while the platform has no mainnet activation height.
    if network == "mainnet" && (TX_COMMANDS.contains(&command) || broadcast) {
        assert!(MAINNET_ACTIVATION_HEIGHT.is_none());
        return Err(
            "mainnet DEX operations are refused: MAINNET_ACTIVATION_HEIGHT is None \
             (frozen platform gate). No real ZEC may be used."
                .to_string(),
        );
    }
    if broadcast {
        return Err(
            "broadcast is BLOCKED-UPSTREAM: the frozen Zalkanes v0 host ABI cannot \
             execute DEX contracts; only --dry-run is available (the default)."
                .to_string(),
        );
    }

    match command {
        "pools" | "pool" | "quote" => {
            if !demo {
                return Err(format!(
                    "'{command}' needs a live indexer transport, which is \
                     BLOCKED-UPSTREAM; use --demo for a deterministic \
                     in-memory demonstration chain"
                ));
            }
            run_demo_view(command, &positional[1..])
        }
        "create-pool" => {
            let [_, factory, token_a, token_b, amt_a, amt_b] = positional[..] else {
                return Err(
                    "usage: create-pool <FACTORY> <TOKEN_A> <TOKEN_B> <AMT_A> <AMT_B>".into(),
                );
            };
            let plan = build_create_pool(&CreatePoolIntent {
                factory: ContractId(parse_id(factory)?),
                token_a: AssetId(parse_id(token_a)?),
                token_b: AssetId(parse_id(token_b)?),
                amount_a: parse_amount(amt_a)?,
                amount_b: parse_amount(amt_b)?,
            })
            .map_err(|e| e.to_string())?;
            Ok(render_plan(&plan))
        }
        "add-liquidity" => {
            let [_, pool, t0, t1, d0, d1, min_lp, expiry] = positional[..] else {
                return Err(
                    "usage: add-liquidity <POOL> <T0> <T1> <D0> <D1> <MIN_LP> <EXPIRY>".into(),
                );
            };
            let plan = build_add_liquidity(&AddLiquidityIntent {
                pool: ContractId(parse_id(pool)?),
                token0: AssetId(parse_id(t0)?),
                token1: AssetId(parse_id(t1)?),
                desired0: parse_amount(d0)?,
                desired1: parse_amount(d1)?,
                min_lp_out: parse_amount(min_lp)?,
                expiry_height: parse_height(expiry)?,
            })
            .map_err(|e| e.to_string())?;
            Ok(render_plan(&plan))
        }
        "remove-liquidity" => {
            let [_, pool, lp, min0, min1, expiry] = positional[..] else {
                return Err("usage: remove-liquidity <POOL> <LP> <MIN0> <MIN1> <EXPIRY>".into());
            };
            let plan = build_remove_liquidity(&RemoveLiquidityIntent {
                pool: ContractId(parse_id(pool)?),
                lp_amount: parse_amount(lp)?,
                min_amount0: parse_amount(min0)?,
                min_amount1: parse_amount(min1)?,
                expiry_height: parse_height(expiry)?,
            })
            .map_err(|e| e.to_string())?;
            Ok(render_plan(&plan))
        }
        "swap" => {
            let [_, pool, token_in, amount, min_out, expiry] = positional[..] else {
                return Err("usage: swap <POOL> <TOKEN_IN> <AMOUNT> <MIN_OUT> <EXPIRY>".into());
            };
            let plan = build_swap_exact_in(&SwapIntent {
                pool: ContractId(parse_id(pool)?),
                incoming_asset: AssetId(parse_id(token_in)?),
                incoming_amount: parse_amount(amount)?,
                min_amount_out: parse_amount(min_out)?,
                expiry_height: parse_height(expiry)?,
            })
            .map_err(|e| e.to_string())?;
            Ok(render_plan(&plan))
        }
        other => Err(format!("unknown command: {other}")),
    }
}

/// Deterministic in-memory demonstration chain for the view commands.
fn run_demo_view(command: &str, rest: &[&str]) -> Result<String, String> {
    use zalkanes_dex_core::encode::{
        factory_create_pool_args, factory_initialize_args, factory_op, token_initialize_args,
        token_op,
    };
    use zalkanes_dex_core::types::Holder;
    use zalkanes_dex_testkit::{account, ContractKind, DexChain};

    let mut chain = DexChain::new();
    let alice = Holder::External(account("demo"));
    let token_a = chain.deploy(ContractKind::TestToken);
    let token_b = chain.deploy(ContractKind::TestToken);
    let (za, zb) = (
        AssetId::of_contract(&token_a),
        AssetId::of_contract(&token_b),
    );
    for (token, name) in [
        (token_a, "Zalkanes Test Asset A"),
        (token_b, "Zalkanes Test Asset B"),
    ] {
        chain
            .call(
                alice,
                token,
                token_op::INITIALIZE,
                &token_initialize_args(name),
                &[],
            )
            .map_err(|e| e.to_string())?;
    }
    chain.faucet(alice, za, 100_000_000);
    chain.faucet(alice, zb, 100_000_000);
    let factory = chain.deploy(ContractKind::SubfrostFactory);
    chain
        .call(
            alice,
            factory,
            factory_op::INITIALIZE,
            &factory_initialize_args(&ContractKind::SubfrostPool.template_hash()),
            &[],
        )
        .map_err(|e| e.to_string())?;
    chain
        .call(
            alice,
            factory,
            factory_op::CREATE_POOL,
            &factory_create_pool_args(&za, &zb),
            &[(za, 5_000_000), (zb, 20_000_000)],
        )
        .map_err(|e| e.to_string())?;

    let view = DexView::new(&chain, factory);
    match command {
        "pools" => {
            let pools = view.get_all_pools().map_err(|e| e.to_string())?;
            let mut out = format!("demo factory: {factory}\npools ({}):\n", pools.len());
            for pool in pools {
                out.push_str(&format!("  {pool}\n"));
            }
            out.push_str(&format!("demo assets:\n  ZA {za}\n  ZB {zb}"));
            Ok(out)
        }
        "pool" => {
            let pool = match rest.first() {
                Some(hex_id) => ContractId(parse_id(hex_id)?),
                None => view
                    .get_all_pools()
                    .map_err(|e| e.to_string())?
                    .first()
                    .copied()
                    .ok_or("no pools")?,
            };
            let details = view.pool_details(pool).map_err(|e| e.to_string())?;
            Ok(format!(
                "pool:      {}\nfactory:   {}\ntoken0:    {}\ntoken1:    {}\nreserve0:  {}\n\
                 reserve1:  {}\nlp_supply: {}\npfees0:    {}\npfees1:    {}",
                details.pool_id,
                details.factory_id,
                details.token0,
                details.token1,
                details.reserve0,
                details.reserve1,
                details.total_lp_supply,
                details.protocol_fees0,
                details.protocol_fees1
            ))
        }
        "quote" => {
            let amount = parse_amount(rest.first().copied().unwrap_or("100000"))?;
            let pool = view
                .get_all_pools()
                .map_err(|e| e.to_string())?
                .first()
                .copied()
                .ok_or("no pools")?;
            let quote = view
                .quote_exact_in(pool, za, amount)
                .map_err(|e| e.to_string())?;
            Ok(format!(
                "quote swap {} ZA:\n  amount_out:   {}\n  total_fee:    {}\n  lp_fee:       {}\n  protocol_fee: {}",
                amount, quote.amount_out, quote.total_fee, quote.lp_fee, quote.protocol_fee
            ))
        }
        _ => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use super::run;

    const POOL: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const TOK_A: &str = "0101010101010101010101010101010101010101010101010101010101010101";
    const TOK_B: &str = "0202020202020202020202020202020202020202020202020202020202020202";

    #[test]
    fn mainnet_dex_operations_are_refused() {
        // §49: every DEX tx command + mainnet must fail, flags or env.
        let swap = ["swap", POOL, TOK_A, "1000", "1", "0"];
        for (args, env) in [
            ([&["--network", "mainnet"][..], &swap[..]].concat(), None),
            (swap.to_vec(), Some("mainnet")),
        ] {
            let err = run(&args, env).expect_err("mainnet must be refused");
            assert!(err.contains("MAINNET_ACTIVATION_HEIGHT"), "{err}");
        }
        let create = [
            "--network",
            "mainnet",
            "create-pool",
            POOL,
            TOK_A,
            TOK_B,
            "1000",
            "1000",
        ];
        assert!(run(&create, None).is_err());
    }

    #[test]
    fn broadcast_is_blocked_upstream_even_off_mainnet() {
        let args = [
            "--network",
            "testnet",
            "--broadcast",
            "swap",
            POOL,
            TOK_A,
            "1000",
            "1",
            "0",
        ];
        let err = run(&args, None).expect_err("broadcast blocked");
        assert!(err.contains("BLOCKED-UPSTREAM"), "{err}");
    }

    #[test]
    fn default_is_dry_run() {
        let args = ["swap", POOL, TOK_A, "1000", "1", "0"];
        let out = run(&args, None).expect("dry run works");
        assert!(out.starts_with("DRY RUN"), "{out}");
        assert!(out.contains("no transaction was created"));
    }

    #[test]
    fn demo_views_answer() {
        assert!(run(&["pools", "--demo"], None)
            .unwrap()
            .contains("pools (1)"));
        assert!(run(&["pool", "--demo"], None)
            .unwrap()
            .contains("lp_supply"));
        assert!(run(&["quote", "--demo", "250000"], None)
            .unwrap()
            .contains("protocol_fee"));
    }
}

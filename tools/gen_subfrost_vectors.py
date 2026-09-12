#!/usr/bin/env python3
"""SUBFROST AMM v0 golden-vector generator.

Independent reference implementation of the frozen v0 arithmetic using
Python's arbitrary-precision integers (math.isqrt for the square root).
This code intentionally shares no code with the Rust implementation
under test.

Emits:
  test-vectors/subfrost_amm_v0_vectors.json   (canonical artifact)
  crates/zalkanes-dex-core/tests/golden_data.rs (generated Rust table)
"""

import json
import math
import os
import sys

FEE_DEN = 10_000
TOTAL_BPS = 100
LP_BPS = 80
MIN_LIQ = 1_000
U128_MAX = (1 << 128) - 1

# Error codes must match crates/zalkanes-dex-core/src/error.rs exactly.
ERR = {
    "NotInitialized": 2,
    "ZeroAmount": 8,
    "InsufficientInitialLiquidity": 9,
    "InsufficientLiquidity": 10,
    "InsufficientLp": 11,
    "ArithmeticOverflow": 14,
}


def fit(x):
    if x > U128_MAX:
        raise OverflowError
    return x


def split_fee(amount_in):
    total = amount_in * TOTAL_BPS // FEE_DEN
    lp = amount_in * LP_BPS // FEE_DEN
    return total, lp, total - lp


def ref_initialize(a0, a1):
    if a0 == 0 or a1 == 0:
        return {"error": "ZeroAmount"}
    gross = math.isqrt(a0 * a1)
    if gross <= MIN_LIQ:
        return {"error": "InsufficientInitialLiquidity"}
    return {
        "provider_lp": gross - MIN_LIQ,
        "locked_lp": MIN_LIQ,
        "total_lp_supply": gross,
        "reserve0": a0,
        "reserve1": a1,
    }


def ref_add(r0, r1, supply, d0, d1):
    if r0 == 0 or r1 == 0 or supply == 0:
        return {"error": "NotInitialized"}
    if d0 == 0 or d1 == 0:
        return {"error": "ZeroAmount"}
    # Upstream branch rule: token1-optimal first.
    optimal1 = d0 * r1 // r0
    if optimal1 <= d1:
        a0, a1 = d0, optimal1
    else:
        a0, a1 = d1 * r0 // r1, d1
    lp = min(a0 * supply // r0, a1 * supply // r1)
    if lp == 0:
        return {"error": "InsufficientLiquidity"}
    try:
        return {
            "accepted0": a0,
            "accepted1": a1,
            "refund0": d0 - a0,
            "refund1": d1 - a1,
            "lp_minted": lp,
            "reserve0": fit(r0 + a0),
            "reserve1": fit(r1 + a1),
            "total_lp_supply": fit(supply + lp),
        }
    except OverflowError:
        return {"error": "ArithmeticOverflow"}


def ref_remove(r0, r1, supply, lp):
    if supply == 0:
        return {"error": "NotInitialized"}
    if lp == 0:
        return {"error": "ZeroAmount"}
    if lp > supply:
        return {"error": "InsufficientLp"}
    a0 = lp * r0 // supply
    a1 = lp * r1 // supply
    if a0 == 0 or a1 == 0:
        return {"error": "InsufficientLiquidity"}
    return {
        "amount0": a0,
        "amount1": a1,
        "reserve0": r0 - a0,
        "reserve1": r1 - a1,
        "total_lp_supply": supply - lp,
    }


def ref_swap(r_in, r_out, amount_in):
    if r_in == 0 or r_out == 0:
        return {"error": "InsufficientLiquidity"}
    if amount_in == 0:
        return {"error": "ZeroAmount"}
    total, lp, proto = split_fee(amount_in)
    if total == 0:
        return {"error": "ZeroAmount"}
    # Denominator-first upstream pricing, generalized to bps.
    wf = amount_in * (FEE_DEN - TOTAL_BPS)
    if wf > U128_MAX:
        return {"error": "ArithmeticOverflow"}
    den = r_in * FEE_DEN + wf
    if r_in * FEE_DEN > U128_MAX or den > U128_MAX:
        return {"error": "ArithmeticOverflow"}
    out = wf * r_out // den
    if out > U128_MAX:
        return {"error": "ArithmeticOverflow"}
    if out == 0:
        return {"error": "InsufficientLiquidity"}
    try:
        return {
            "amount_out": out,
            "total_fee": total,
            "lp_fee": lp,
            "protocol_fee": proto,
            "reserve_in": fit(r_in + amount_in - proto),
            "reserve_out": r_out - out,
            "protocol_fee_accrued_in": proto,
        }
    except OverflowError:
        return {"error": "ArithmeticOverflow"}


def main(out_json, out_rs):
    vectors = []

    def add(vid, op, args, expected):
        vectors.append({"id": vid, "op": op, "args": args, "expected": expected})

    # ── initialization (10 valid + failures below) ───────────────────────
    init_cases = [
        (1_000_000, 1_000_000),
        (1_002, 1_001),
        (5_000_000, 20_000_000),
        (1, 10_000_000_000_000),
        (123_456_789, 987_654_321),
        (10**18, 10**18),
        (2**100, 2**100),          # wide-product sqrt path
        (2**64 - 1, 2**64 + 1),
        (3, 400_000_007),
        (999_983, 2_750_161),
    ]
    for i, (a0, a1) in enumerate(init_cases):
        add(f"INIT-{i+1:03}", "initialize", {"amount0": a0, "amount1": a1},
            ref_initialize(a0, a1))

    # ── add liquidity ────────────────────────────────────────────────────
    add_cases = [
        (1_000_000, 1_000_000, 1_000_000, 500_000, 500_000),
        (1_000_000, 1_000_000, 1_000_000, 500_000, 700_000),
        (1_000_000, 1_000_000, 1_000_000, 700_000, 500_000),
        (1_000_000, 4_000_000, 2_000_000, 100_000, 400_001),
        (7, 13, 1_009, 3, 5),
        (10**18, 3 * 10**18, 10**18, 10**12, 10**12),
        (999_983, 2_750_161, 1_657_349, 123_457, 339_559),
        (2**90, 2**90, 2**90, 2**80, 2**80 + 12345),
        (1_000_000, 1_000_000, 1_000_000, 1, 1),
        (5_000, 5_000, 5_000, 4_999, 5_001),
    ]
    for i, (r0, r1, s, d0, d1) in enumerate(add_cases):
        add(f"ADD-{i+1:03}", "add_liquidity",
            {"reserve0": r0, "reserve1": r1, "total_lp_supply": s,
             "desired0": d0, "desired1": d1},
            ref_add(r0, r1, s, d0, d1))

    # ── remove liquidity ─────────────────────────────────────────────────
    remove_cases = [
        (1_000_000, 1_000_000, 1_000_000, 500_000),
        (1_000_000, 1_000_000, 1_000_000, 1),
        (999_983, 2_750_161, 1_657_349, 657_349),
        (10**18, 3 * 10**18, 10**18, 10**17),
        (7, 13, 1_009, 9),
        (2**100, 2**100, 2**100, 2**99),
        (1_000_000, 4_000_000, 2_000_000, 1_999_000),   # all circulating LP
        (5_000, 5_000, 5_000, 4_999),
        (123_456_789, 987_654_321, 349_206_090, 349_206_089),
        (1_000_001, 999_999, 1_000_000, 333_333),
    ]
    for i, (r0, r1, s, lp) in enumerate(remove_cases):
        add(f"REM-{i+1:03}", "remove_liquidity",
            {"reserve0": r0, "reserve1": r1, "total_lp_supply": s,
             "lp_amount": lp},
            ref_remove(r0, r1, s, lp))

    # ── swaps ────────────────────────────────────────────────────────────
    swap_cases = [
        (1_000_000, 1_000_000, 10_000),
        (1_000_000, 1_000_000, 100),           # exact fee boundary
        (1_000_000, 1_000_000, 99),            # below one fee unit
        (1_000_000, 1_000_000, 101),           # just above boundary
        (1_000_000, 1_000_000, 1_000_000),     # large price impact
        (2_750_161, 999_983, 137_251),
        (999_983, 2_750_161, 137_251),
        (10**18, 10**18, 10**15),
        (10**30, 10**30, 10**27),              # wide intermediate required
        (2**110, 2**110, 2**100),              # near-max wide case
        (1_000_000_000, 3, 1_000_000),         # tiny out reserve
        (5, 5_000_000_000, 1_000),
        (123_456_789, 987_654_321, 5_000_000),
        (1_000_000, 1_000_000, 12_345),
        (7_777_777, 3_333_333, 999_999),
    ]
    for i, (ri, ro, ain) in enumerate(swap_cases):
        add(f"SWAP-{i+1:03}", "swap_exact_in",
            {"reserve_in": ri, "reserve_out": ro, "amount_in": ain},
            ref_swap(ri, ro, ain))

    # ── failures ─────────────────────────────────────────────────────────
    fail_cases = [
        ("FAIL-001", "initialize", {"amount0": 1_000, "amount1": 1_000},
         ref_initialize(1_000, 1_000)),                       # exactly minimum
        ("FAIL-002", "initialize", {"amount0": 0, "amount1": 10_000},
         ref_initialize(0, 10_000)),
        ("FAIL-003", "swap_exact_in",
         {"reserve_in": 1_000_000, "reserve_out": 1_000_000, "amount_in": 0},
         ref_swap(1_000_000, 1_000_000, 0)),
        ("FAIL-004", "swap_exact_in",
         {"reserve_in": 0, "reserve_out": 1_000_000, "amount_in": 10_000},
         ref_swap(0, 1_000_000, 10_000)),
        ("FAIL-005", "swap_exact_in",
         {"reserve_in": 1_000_000, "reserve_out": 1_000_000, "amount_in": 50},
         ref_swap(1_000_000, 1_000_000, 50)),                 # fee rounds to 0
        ("FAIL-006", "remove_liquidity",
         {"reserve0": 1_000, "reserve1": 1_000, "total_lp_supply": 1_000,
          "lp_amount": 1_001},
         ref_remove(1_000, 1_000, 1_000, 1_001)),
        ("FAIL-007", "add_liquidity",
         {"reserve0": 10**30, "reserve1": 10**30, "total_lp_supply": 10**30,
          "desired0": 0, "desired1": 10},
         ref_add(10**30, 10**30, 10**30, 0, 10)),
        ("FAIL-008", "swap_exact_in",
         {"reserve_in": U128_MAX - 1, "reserve_out": 1_000_000,
          "amount_in": 1_000_000},
         ref_swap(U128_MAX - 1, 1_000_000, 1_000_000)),       # overflow path
    ]
    for vid, op, args, expected in fail_cases:
        add(vid, op, args, expected)

    for v in vectors:
        for k, val in list(v["args"].items()):
            assert val <= U128_MAX, f"{v['id']} arg {k} exceeds u128"
        for k, val in list(v["expected"].items()):
            if isinstance(val, int):
                assert val <= U128_MAX, f"{v['id']} expected {k} exceeds u128"

    with open(out_json, "w") as f:
        json.dump(
            {
                "schema": "subfrost-amm-v0-vectors",
                "generator": "tools/gen_subfrost_vectors.py (python bigint reference)",
                "constants": {
                    "FEE_DENOMINATOR_BPS": FEE_DEN,
                    "TOTAL_SWAP_FEE_BPS": TOTAL_BPS,
                    "LP_FEE_BPS": LP_BPS,
                    "PROTOCOL_FEE_BPS": TOTAL_BPS - LP_BPS,
                    "MINIMUM_LIQUIDITY": MIN_LIQ,
                },
                "vectors": vectors,
            },
            f,
            indent=1,
            sort_keys=True,
        )
        f.write("\n")

    def rs_u128(x):
        return f"{x}u128"

    lines = [
        "// @generated by tools/gen_subfrost_vectors.py — DO NOT EDIT BY HAND.",
        "// Canonical artifact: test-vectors/subfrost_amm_v0_vectors.json",
        "#![allow(clippy::unreadable_literal)]",
        "",
        "pub enum Expected {",
        "    Initialize { provider_lp: u128, locked_lp: u128, total_lp_supply: u128 },",
        "    Add { accepted0: u128, accepted1: u128, refund0: u128, refund1: u128, lp_minted: u128 },",
        "    Remove { amount0: u128, amount1: u128 },",
        "    Swap { amount_out: u128, total_fee: u128, lp_fee: u128, protocol_fee: u128 },",
        "    Error(u32),",
        "}",
        "",
        "pub struct Vector {",
        "    pub id: &'static str,",
        "    pub op: &'static str,",
        "    pub args: [u128; 5],",
        "    pub expected: Expected,",
        "}",
        "",
        "pub const VECTORS: &[Vector] = &[",
    ]
    for v in vectors:
        a = v["args"]
        op = v["op"]
        if op == "initialize":
            args = [a["amount0"], a["amount1"], 0, 0, 0]
        elif op == "add_liquidity":
            args = [a["reserve0"], a["reserve1"], a["total_lp_supply"],
                    a["desired0"], a["desired1"]]
        elif op == "remove_liquidity":
            args = [a["reserve0"], a["reserve1"], a["total_lp_supply"],
                    a["lp_amount"], 0]
        else:
            args = [a["reserve_in"], a["reserve_out"], a["amount_in"], 0, 0]
        e = v["expected"]
        if "error" in e:
            exp = f"Expected::Error({ERR[e['error']]})"
        elif op == "initialize":
            exp = (f"Expected::Initialize {{ provider_lp: {rs_u128(e['provider_lp'])}, "
                   f"locked_lp: {rs_u128(e['locked_lp'])}, "
                   f"total_lp_supply: {rs_u128(e['total_lp_supply'])} }}")
        elif op == "add_liquidity":
            exp = (f"Expected::Add {{ accepted0: {rs_u128(e['accepted0'])}, "
                   f"accepted1: {rs_u128(e['accepted1'])}, "
                   f"refund0: {rs_u128(e['refund0'])}, "
                   f"refund1: {rs_u128(e['refund1'])}, "
                   f"lp_minted: {rs_u128(e['lp_minted'])} }}")
        elif op == "remove_liquidity":
            exp = (f"Expected::Remove {{ amount0: {rs_u128(e['amount0'])}, "
                   f"amount1: {rs_u128(e['amount1'])} }}")
        else:
            exp = (f"Expected::Swap {{ amount_out: {rs_u128(e['amount_out'])}, "
                   f"total_fee: {rs_u128(e['total_fee'])}, "
                   f"lp_fee: {rs_u128(e['lp_fee'])}, "
                   f"protocol_fee: {rs_u128(e['protocol_fee'])} }}")
        args_s = ", ".join(rs_u128(x) for x in args)
        lines.append(
            f'    Vector {{ id: "{v["id"]}", op: "{op}", '
            f"args: [{args_s}], expected: {exp} }},"
        )
    lines.append("];")
    lines.append("")
    with open(out_rs, "w") as f:
        f.write("\n".join(lines))

    print(f"wrote {len(vectors)} vectors -> {out_json}, {out_rs}")


if __name__ == "__main__":
    root = sys.argv[1] if len(sys.argv) > 1 else "."
    main(
        os.path.join(root, "test-vectors", "subfrost_amm_v0_vectors.json"),
        os.path.join(root, "crates", "zalkanes-dex-core", "tests", "golden", "data.rs"),
    )

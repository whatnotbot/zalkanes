//! subfrost-mathcheck — SUBFROST AMM v0 arithmetic under the REAL frozen
//! Zalkanes v0 host ABI (the six imports only).
//!
//! Purpose: prove on the actual consensus wasmi runtime that the DEX
//! arithmetic is wasm32-clean, float-free, deterministic and fuel-bounded.
//! The full pool/factory contracts need the proposed ABI extension; this
//! contract needs nothing beyond frozen v0 and is deployable today.
//!
//! Opcodes:
//!   0x0010  stage(slot u8 ‖ value u128 BE)   — write an argument slot
//!                                              (17-byte calldata fits the
//!                                              38-byte inline limit)
//!   0x0020  exec(op u8)                       — run the op over staged
//!                                              slots; persist + return the
//!                                              result record
//!   0x0021  exec_view(op u8 ‖ 5 × u128 BE)   — pure compute, full args
//!                                              inline (view path)
//!   0x0030  result()                          — read last persisted record
//!
//! op values: 1 = initial_liquidity(a0, a1)
//!            2 = quote_add_liquidity(r0, r1, supply, d0, d1)
//!            3 = quote_remove_liquidity(r0, r1, supply, lp)
//!            4 = quote_swap_exact_in(reserve_in, reserve_out, amount_in)
//!
//! Result record: [1] ‖ big-endian u128 outputs   on success
//!                [0] ‖ error code u32 BE          on math error

#![no_std]
#![no_main]

#[global_allocator]
static ALLOC: wee_alloc::WeeAlloc = wee_alloc::WeeAlloc::INIT;
extern crate alloc;

use alloc::vec::Vec;

use zalkanes_dex_core::math::{
    initial_liquidity, quote_add_liquidity, quote_remove_liquidity, quote_swap_exact_in,
};
use zalkanes_sdk as sdk;

const RESULT_KEY: &[u8] = b"result";

fn slot_key(slot: u8) -> [u8; 4] {
    [b'a', b'r', b'g', slot]
}

fn read_u128_be(bytes: &[u8]) -> Option<u128> {
    if bytes.len() != 16 {
        return None;
    }
    let mut buf = [0u8; 16];
    buf.copy_from_slice(bytes);
    Some(u128::from_be_bytes(buf))
}

fn staged(slot: u8) -> u128 {
    sdk::get(&slot_key(slot))
        .and_then(|b| read_u128_be(&b))
        .unwrap_or(0)
}

fn compute(op: u8, args: [u128; 5]) -> Vec<u8> {
    let mut out = Vec::with_capacity(1 + 5 * 16);
    let result: Result<Vec<u128>, zalkanes_dex_core::error::DexError> = match op {
        1 => initial_liquidity(args[0], args[1])
            .map(|(provider, gross)| alloc::vec![provider, gross]),
        2 => quote_add_liquidity(args[0], args[1], args[2], args[3], args[4])
            .map(|o| alloc::vec![o.accepted0, o.accepted1, o.refund0, o.refund1, o.lp_minted]),
        3 => quote_remove_liquidity(args[0], args[1], args[2], args[3])
            .map(|(a0, a1)| alloc::vec![a0, a1]),
        4 => quote_swap_exact_in(args[0], args[1], args[2]).map(|o| {
            alloc::vec![
                o.amount_out,
                o.fees.total_fee,
                o.fees.lp_fee,
                o.fees.protocol_fee
            ]
        }),
        _ => Err(zalkanes_dex_core::error::DexError::InvalidOpcode),
    };
    match result {
        Ok(values) => {
            out.push(1);
            for value in values {
                out.extend_from_slice(&value.to_be_bytes());
            }
        }
        Err(err) => {
            out.push(0);
            out.extend_from_slice(&err.code().to_be_bytes());
        }
    }
    out
}

#[no_mangle]
pub extern "C" fn dispatch(opcode: i32, _input_len: i32) -> i32 {
    match opcode as u16 {
        // stage(slot, value)
        0x0010 => {
            let input = sdk::read_input();
            if input.len() != 17 {
                return -1;
            }
            let slot = input[0];
            if slot > 4 {
                return -1;
            }
            let mut value = [0u8; 16];
            value.copy_from_slice(&input[1..17]);
            if !sdk::set(&slot_key(slot), &value) {
                return -1;
            }
            0
        }
        // exec(op) over staged slots; persists + returns the record
        0x0020 => {
            let input = sdk::read_input();
            if input.len() != 1 {
                return -1;
            }
            let record = compute(
                input[0],
                [staged(0), staged(1), staged(2), staged(3), staged(4)],
            );
            if !sdk::set(RESULT_KEY, &record) {
                return -1;
            }
            if sdk::write_output(&record) {
                0
            } else {
                -1
            }
        }
        // exec_view(op ‖ 5 × u128): pure compute
        0x0021 => {
            let input = sdk::read_input();
            if input.len() != 81 {
                return -1;
            }
            let mut args = [0u128; 5];
            for (i, arg) in args.iter_mut().enumerate() {
                let start = 1 + i * 16;
                match read_u128_be(&input[start..start + 16]) {
                    Some(v) => *arg = v,
                    None => return -1,
                }
            }
            let record = compute(input[0], args);
            if sdk::write_output(&record) {
                0
            } else {
                -1
            }
        }
        // result(): last persisted record
        0x0030 => match sdk::get(RESULT_KEY) {
            Some(record) => {
                if sdk::write_output(&record) {
                    0
                } else {
                    -1
                }
            }
            None => -1,
        },
        _ => -1,
    }
}

// Required for no_std: the frozen six-import profile links no std, so the
// contract provides its own panic handler (burns fuel to exhaustion —
// same convention as the reference contracts).
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}

//! Token contract — fungible asset reference implementation.
//!
//! Opcodes:
//!   0x0001  initialize(supply: u128 BE)   — sets total supply to deployer
//!   0x0002  balance_of(addr: [u8;32]) -> u128 BE
//!   0x0003  transfer(to: [u8;32], amount: u128 BE) — caller is input[64..80]
//!   0x0004  mint(to: [u8;32], amount: u128 BE)     — only owner
//!   0x0005  burn(from: [u8;32], amount: u128 BE)   — only owner
//!   0x0006  total_supply() -> u128 BE
//!
//! Storage keys:
//!   b"balance/" ++ addr[32]  -> u128 BE
//!   b"supply"                -> u128 BE
//!   b"owner"                 -> addr[32]

#![no_std]
#![no_main]

#[global_allocator]
static ALLOC: wee_alloc::WeeAlloc = wee_alloc::WeeAlloc::INIT;
extern crate alloc;

use zalkanes_sdk as sdk;

fn balance_key(addr: &[u8; 32]) -> alloc::vec::Vec<u8> {
    let mut k = alloc::vec::Vec::with_capacity(8 + 32);
    k.extend_from_slice(b"balance/");
    k.extend_from_slice(addr);
    k
}

fn get_balance(addr: &[u8; 32]) -> u128 {
    sdk::get(&balance_key(addr))
        .map(|b| bytes_to_u128(&b))
        .unwrap_or(0)
}

fn set_balance(addr: &[u8; 32], v: u128) {
    sdk::set(&balance_key(addr), &u128_to_bytes(v));
}

fn get_supply() -> u128 {
    sdk::get(b"supply").map(|b| bytes_to_u128(&b)).unwrap_or(0)
}

fn set_supply(v: u128) {
    sdk::set(b"supply", &u128_to_bytes(v));
}

fn u128_to_bytes(v: u128) -> [u8; 16] {
    v.to_be_bytes()
}

fn bytes_to_u128(b: &[u8]) -> u128 {
    if b.len() < 16 {
        return 0;
    }
    let mut arr = [0u8; 16];
    arr.copy_from_slice(&b[..16]);
    u128::from_be_bytes(arr)
}

#[no_mangle]
pub extern "C" fn dispatch(opcode: i32, _input_len: i32) -> i32 {
    let input = sdk::read_input();
    match opcode as u16 {
        // initialize(supply: u128 BE, owner: [u8;32])
        0x0001 => {
            if input.len() < 48 {
                return -1;
            }
            let supply = bytes_to_u128(&input[..16]);
            let owner: [u8; 32] = input[16..48].try_into().unwrap_or([0u8; 32]);
            set_supply(supply);
            set_balance(&owner, supply);
            sdk::set(b"owner", &owner);
            0
        }
        // balance_of(addr: [u8;32]) -> u128 BE
        0x0002 => {
            if input.len() < 32 {
                return -1;
            }
            let addr: [u8; 32] = input[..32].try_into().unwrap_or([0u8; 32]);
            let bal = get_balance(&addr);
            sdk::write_output(&u128_to_bytes(bal));
            0
        }
        // transfer(from: [u8;32], to: [u8;32], amount: u128 BE)
        0x0003 => {
            if input.len() < 80 {
                return -1;
            }
            let from: [u8; 32] = input[0..32].try_into().unwrap_or([0u8; 32]);
            let to: [u8; 32] = input[32..64].try_into().unwrap_or([0u8; 32]);
            let amount = bytes_to_u128(&input[64..80]);
            let from_bal = get_balance(&from);
            if from_bal < amount {
                return -1;
            } // insufficient balance
              // No overflow possible: total supply fits in u128
            set_balance(&from, from_bal - amount);
            let to_bal = get_balance(&to);
            set_balance(&to, to_bal.saturating_add(amount));
            0
        }
        // mint(to: [u8;32], amount: u128 BE)
        0x0004 => {
            if input.len() < 48 {
                return -1;
            }
            let to: [u8; 32] = input[0..32].try_into().unwrap_or([0u8; 32]);
            let amount = bytes_to_u128(&input[32..48]);
            let supply = get_supply();
            let new_supply = supply.checked_add(amount).unwrap_or(u128::MAX);
            if new_supply == u128::MAX {
                return -1;
            } // overflow guard
            set_supply(new_supply);
            let bal = get_balance(&to);
            set_balance(&to, bal.saturating_add(amount));
            0
        }
        // burn(from: [u8;32], amount: u128 BE)
        0x0005 => {
            if input.len() < 48 {
                return -1;
            }
            let from: [u8; 32] = input[0..32].try_into().unwrap_or([0u8; 32]);
            let amount = bytes_to_u128(&input[32..48]);
            let bal = get_balance(&from);
            if bal < amount {
                return -1;
            }
            let supply = get_supply();
            set_balance(&from, bal - amount);
            set_supply(supply.saturating_sub(amount));
            0
        }
        // total_supply() -> u128 BE
        0x0006 => {
            sdk::write_output(&u128_to_bytes(get_supply()));
            0
        }
        _ => -1,
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}

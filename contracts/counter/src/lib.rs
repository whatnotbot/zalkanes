//! Counter contract — reference implementation.
//!
//! Opcodes:
//!   0x0001  initialize(initial_value: u64)
//!   0x0002  increment()
//!   0x0003  get() -> u64 (big-endian)

#![no_std]
#![no_main]

#[global_allocator]
static ALLOC: wee_alloc::WeeAlloc = wee_alloc::WeeAlloc::INIT;
extern crate alloc;

use zalkanes_sdk as sdk;

const KEY: &[u8] = b"counter";

#[no_mangle]
pub extern "C" fn dispatch(opcode: i32, _input_len: i32) -> i32 {
    match opcode as u16 {
        // initialize(initial_value: u64)
        0x0001 => {
            let input = sdk::read_input();
            if input.len() < 8 {
                return -1;
            }
            let val = sdk::bytes_to_u64(&input[..8]);
            sdk::set(KEY, &sdk::u64_to_bytes(val));
            0
        }
        // increment()
        0x0002 => {
            let current = sdk::get(KEY).map(|b| sdk::bytes_to_u64(&b)).unwrap_or(0);
            let next = current.wrapping_add(1);
            sdk::set(KEY, &sdk::u64_to_bytes(next));
            0
        }
        // get() -> big-endian u64
        0x0003 => {
            let val = sdk::get(KEY).map(|b| sdk::bytes_to_u64(&b)).unwrap_or(0);
            let bytes = sdk::u64_to_bytes(val);
            if sdk::write_output(&bytes) {
                0
            } else {
                -1
            }
        }
        _ => -1,
    }
}

// Required for no_std panic handler
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}

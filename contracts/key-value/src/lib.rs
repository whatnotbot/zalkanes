//! Key-Value store contract.
//!
//! Opcodes:
//!   0x0001  set(key_len: u16, key: bytes, value: bytes)
//!   0x0002  get(key: bytes) -> value bytes
//!   0x0003  delete(key: bytes)

#![no_std]
#![no_main]

#[global_allocator]
static ALLOC: wee_alloc::WeeAlloc = wee_alloc::WeeAlloc::INIT;
extern crate alloc;
use zalkanes_sdk as sdk;

#[no_mangle]
pub extern "C" fn dispatch(opcode: i32, _input_len: i32) -> i32 {
    let input = sdk::read_input();
    match opcode as u16 {
        // set(key_len: u16 BE, key: bytes, value: bytes)
        0x0001 => {
            if input.len() < 2 {
                return -1;
            }
            let key_len = u16::from_be_bytes([input[0], input[1]]) as usize;
            if input.len() < 2 + key_len {
                return -1;
            }
            let key = &input[2..2 + key_len];
            let value = &input[2 + key_len..];
            sdk::set(key, value);
            0
        }
        // get(key: bytes) -> value
        0x0002 => match sdk::get(&input) {
            Some(v) => {
                sdk::write_output(&v);
                0
            }
            None => -1,
        },
        // delete(key: bytes)
        0x0003 => {
            sdk::delete(&input);
            0
        }
        _ => -1,
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}

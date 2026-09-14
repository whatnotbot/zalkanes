//! Caller contract — cross-contract call demo.
//!
//! **Not deployable on protocol v0.** It imports `env::contract_call`, which
//! the v0 host ABI does not provide, so `validate_module` rejects it at
//! deploy time ("unresolvable import env::contract_call"). It is kept as an
//! educational example of the shape a nested call would take and of the
//! trap-rollback pattern (opcode 0x0003). See contracts/caller/README.md.
//!
//! Opcodes:
//!   0x0001  call_counter(counter_id: [u8;32])  — calls increment (0x0001) on target
//!   0x0002  nested_success(target_id: [u8;32]) — calls get (0x0002) on target, returns result
//!   0x0003  nested_failure()                   — traps intentionally for rollback testing

#![no_std]
#![no_main]

#[global_allocator]
static ALLOC: wee_alloc::WeeAlloc = wee_alloc::WeeAlloc::INIT;
extern crate alloc;
use zalkanes_sdk as sdk;

// `contract_call` is NOT a v0 host import. Declaring it here is what makes
// this module fail validation on v0; the declaration documents the shape only.
extern "C" {
    fn contract_call(
        id_ptr: i32,
        opcode: i32,
        in_ptr: i32,
        in_len: i32,
        out_ptr: i32,
        out_max: i32,
    ) -> i32;
}

#[no_mangle]
pub extern "C" fn dispatch(opcode: i32, _input_len: i32) -> i32 {
    let input = sdk::read_input();
    match opcode as u16 {
        // call_counter(target_id: [u8;32])
        0x0001 => {
            if input.len() < 32 {
                return -1;
            }
            let id_ptr = input.as_ptr() as i32;
            let mut out = [0u8; 256];
            let rc = unsafe {
                contract_call(
                    id_ptr,
                    0x0001, // counter increment opcode
                    0,
                    0,
                    out.as_mut_ptr() as i32,
                    out.len() as i32,
                )
            };
            if rc < 0 {
                return -1;
            }
            0
        }
        // nested_success: call get on target, return its output
        0x0002 => {
            if input.len() < 32 {
                return -1;
            }
            let id_ptr = input.as_ptr() as i32;
            let mut out = [0u8; 256];
            let rc = unsafe {
                contract_call(
                    id_ptr,
                    0x0002, // counter get opcode
                    0,
                    0,
                    out.as_mut_ptr() as i32,
                    out.len() as i32,
                )
            };
            if rc < 0 {
                return -1;
            }
            sdk::write_output(&out[..rc as usize]);
            0
        }
        // nested_failure: intentional trap for rollback testing
        0x0003 => {
            sdk::set(b"before_trap", b"written");
            // Trap — storage write above MUST be rolled back
            core::arch::wasm32::unreachable()
        }
        _ => -1,
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}

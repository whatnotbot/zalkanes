//! Caller contract — cross-contract call demo.
//!
//! Opcodes:
//!   0x0001  call_counter(counter_id: [u8;32])  — calls increment on target
//!   0x0002  nested_success(target_id: [u8;32]) — calls get on target, returns result
//!   0x0003  nested_failure()                   — traps intentionally for rollback testing

#![no_std]
#![no_main]
extern crate alloc;
use zalkanes_sdk as sdk;

// Note: contract_call host function is declared here for demo purposes.
// In a real deployment the SDK would export this helper.
extern "C" {
    fn contract_call(
        id_ptr: i32, opcode: i32,
        in_ptr: i32, in_len: i32,
        out_ptr: i32, out_max: i32,
    ) -> i32;
}

#[no_mangle]
pub extern "C" fn dispatch(opcode: i32, _input_len: i32) -> i32 {
    let input = sdk::read_input();
    match opcode as u16 {
        // call_counter(target_id: [u8;32])
        0x0001 => {
            if input.len() < 32 { return -1; }
            let id_ptr = input.as_ptr() as i32;
            let mut out = [0u8; 256];
            let rc = unsafe {
                contract_call(
                    id_ptr, 0x0002, // increment opcode
                    0, 0,
                    out.as_mut_ptr() as i32, out.len() as i32,
                )
            };
            if rc < 0 { return -1; }
            0
        }
        // nested_success: call get on target, return its output
        0x0002 => {
            if input.len() < 32 { return -1; }
            let id_ptr = input.as_ptr() as i32;
            let mut out = [0u8; 256];
            let rc = unsafe {
                contract_call(
                    id_ptr, 0x0003, // get opcode
                    0, 0,
                    out.as_mut_ptr() as i32, out.len() as i32,
                )
            };
            if rc < 0 { return -1; }
            sdk::write_output(&out[..rc as usize]);
            0
        }
        // nested_failure: intentional trap for rollback testing
        0x0003 => {
            sdk::set(b"before_trap", b"written");
            // Trap — storage write above MUST be rolled back
            unsafe { core::arch::wasm32::unreachable() }
        }
        _ => -1,
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}

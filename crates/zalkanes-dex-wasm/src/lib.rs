//! WASM entry glue for DEX contracts against the **proposed** DEX host ABI.
//!
//! The frozen Zalkanes v0 runtime only links the six-import host ABI, so
//! modules produced with this glue are *not deployable* on v0 — they
//! compile (proving the logic is wasm32/no-float/no_std clean) and they
//! define byte-exact bindings for the ABI extension the DEX requires.
//! Deployability is tracked as an upstream blocker in
//! `docs/subfrost-amm-v0.md`.
//!
//! On non-wasm32 targets this crate compiles to an empty library.

#![no_std]
#![allow(unexpected_cfgs)]

#[cfg(target_arch = "wasm32")]
extern crate alloc;

#[cfg(target_arch = "wasm32")]
pub mod wasm {
    use alloc::vec::Vec;
    use core::cell::UnsafeCell;

    use zalkanes_dex_core::error::DexError;
    use zalkanes_dex_core::events::DexEvent;
    use zalkanes_dex_core::host::Host;
    use zalkanes_dex_core::types::{AssetId, ContractId, Holder};

    /// Proposed `env` imports. The first six exist in Zalkanes v0 today;
    /// every `dex_*` function is the requested ABI extension.
    // SAFETY justification for this module's `unsafe`: FFI to host
    // functions whose pointer/length contracts are defined by the ABI
    // proposal; all buffers passed are owned, correctly sized locals.
    #[allow(unsafe_code)]
    mod ffi {
        extern "C" {
            pub fn storage_get(key_ptr: i32, key_len: i32, val_ptr: i32) -> i32;
            pub fn storage_set(key_ptr: i32, key_len: i32, val_ptr: i32, val_len: i32) -> i32;
            pub fn storage_delete(key_ptr: i32, key_len: i32) -> i32;
            pub fn context_block_height(out_ptr: i32) -> i32;
            pub fn input_read(out_ptr: i32, offset: i32, len: i32) -> i32;
            pub fn output_write(ptr: i32, len: i32) -> i32;
            // ── proposed DEX extension ──────────────────────────────
            pub fn dex_self_id(out_ptr: i32) -> i32;
            pub fn dex_caller(out_ptr: i32) -> i32;
            pub fn dex_incoming_count() -> i32;
            pub fn dex_incoming_get(index: i32, out_ptr: i32) -> i32;
            pub fn dex_transfer_out(to_ptr: i32, asset_ptr: i32, amount_ptr: i32) -> i32;
            pub fn dex_mint_own(to_ptr: i32, amount_ptr: i32) -> i32;
            pub fn dex_burn_own(amount_ptr: i32) -> i32;
            pub fn dex_emit_event(ptr: i32, len: i32) -> i32;
            pub fn dex_call(
                target_ptr: i32,
                opcode: i32,
                input_ptr: i32,
                input_len: i32,
                assets_ptr: i32,
                assets_len: i32,
                out_ptr: i32,
                out_cap: i32,
            ) -> i32;
            pub fn dex_spawn(template_ptr: i32, out_ptr: i32) -> i32;
            pub fn dex_consume_fuel(units: i64) -> i32;
        }
    }

    const MAX_VALUE: usize = 65_536;

    fn err_from_code(code: i32) -> DexError {
        u32::try_from(-code)
            .ok()
            .and_then(DexError::from_code)
            .unwrap_or(DexError::InvalidArguments)
    }

    /// `Host` implementation over the proposed ABI.
    pub struct WasmHost;

    #[allow(unsafe_code)] // SAFETY: see `ffi` module note.
    impl Host for WasmHost {
        fn self_id(&self) -> ContractId {
            let mut buf = [0u8; 32];
            unsafe {
                ffi::dex_self_id(buf.as_mut_ptr() as i32);
            }
            ContractId(buf)
        }

        fn caller(&self) -> Holder {
            let mut buf = [0u8; 33];
            unsafe {
                ffi::dex_caller(buf.as_mut_ptr() as i32);
            }
            Holder::from_bytes(&buf).unwrap_or(Holder::External(
                zalkanes_dex_core::types::AccountId([0u8; 32]),
            ))
        }

        fn block_height(&self) -> u32 {
            let mut buf = [0u8; 4];
            unsafe {
                ffi::context_block_height(buf.as_mut_ptr() as i32);
            }
            u32::from_be_bytes(buf)
        }

        fn incoming_assets(&self) -> Vec<(AssetId, u128)> {
            let count = unsafe { ffi::dex_incoming_count() }.max(0) as usize;
            let mut out = Vec::with_capacity(count);
            for i in 0..count {
                let mut buf = [0u8; 48];
                let rc = unsafe { ffi::dex_incoming_get(i as i32, buf.as_mut_ptr() as i32) };
                if rc < 0 {
                    break;
                }
                let mut asset = [0u8; 32];
                asset.copy_from_slice(&buf[..32]);
                let mut amt = [0u8; 16];
                amt.copy_from_slice(&buf[32..48]);
                out.push((AssetId(asset), u128::from_be_bytes(amt)));
            }
            out
        }

        fn storage_get(&self, key: &[u8]) -> Option<Vec<u8>> {
            let mut buf = alloc::vec![0u8; MAX_VALUE];
            let len = unsafe {
                ffi::storage_get(
                    key.as_ptr() as i32,
                    key.len() as i32,
                    buf.as_mut_ptr() as i32,
                )
            };
            if len < 0 {
                return None;
            }
            buf.truncate(len as usize);
            Some(buf)
        }

        fn storage_set(&mut self, key: &[u8], value: &[u8]) -> Result<(), DexError> {
            let rc = unsafe {
                ffi::storage_set(
                    key.as_ptr() as i32,
                    key.len() as i32,
                    value.as_ptr() as i32,
                    value.len() as i32,
                )
            };
            if rc == 0 {
                Ok(())
            } else {
                Err(DexError::InvalidArguments)
            }
        }

        fn storage_delete(&mut self, key: &[u8]) -> Result<(), DexError> {
            let rc = unsafe { ffi::storage_delete(key.as_ptr() as i32, key.len() as i32) };
            if rc == 0 {
                Ok(())
            } else {
                Err(DexError::InvalidArguments)
            }
        }

        fn transfer_out(
            &mut self,
            to: &Holder,
            asset: &AssetId,
            amount: u128,
        ) -> Result<(), DexError> {
            let to_bytes = to.to_bytes();
            let amount_bytes = amount.to_be_bytes();
            let rc = unsafe {
                ffi::dex_transfer_out(
                    to_bytes.as_ptr() as i32,
                    asset.0.as_ptr() as i32,
                    amount_bytes.as_ptr() as i32,
                )
            };
            if rc == 0 {
                Ok(())
            } else {
                Err(err_from_code(rc))
            }
        }

        fn mint_own_asset(&mut self, to: &Holder, amount: u128) -> Result<(), DexError> {
            let to_bytes = to.to_bytes();
            let amount_bytes = amount.to_be_bytes();
            let rc = unsafe {
                ffi::dex_mint_own(to_bytes.as_ptr() as i32, amount_bytes.as_ptr() as i32)
            };
            if rc == 0 {
                Ok(())
            } else {
                Err(err_from_code(rc))
            }
        }

        fn burn_own_asset(&mut self, amount: u128) -> Result<(), DexError> {
            let amount_bytes = amount.to_be_bytes();
            let rc = unsafe { ffi::dex_burn_own(amount_bytes.as_ptr() as i32) };
            if rc == 0 {
                Ok(())
            } else {
                Err(err_from_code(rc))
            }
        }

        fn emit_event(&mut self, event: &DexEvent) {
            let bytes = event.encode();
            unsafe {
                ffi::dex_emit_event(bytes.as_ptr() as i32, bytes.len() as i32);
            }
        }

        fn call(
            &mut self,
            target: &ContractId,
            opcode: u16,
            input: &[u8],
            assets: &[(AssetId, u128)],
        ) -> Result<Vec<u8>, DexError> {
            let mut packed = Vec::with_capacity(assets.len() * 48);
            for (asset, amount) in assets {
                packed.extend_from_slice(&asset.0);
                packed.extend_from_slice(&amount.to_be_bytes());
            }
            let mut out = alloc::vec![0u8; MAX_VALUE];
            let rc = unsafe {
                ffi::dex_call(
                    target.0.as_ptr() as i32,
                    i32::from(opcode),
                    input.as_ptr() as i32,
                    input.len() as i32,
                    packed.as_ptr() as i32,
                    packed.len() as i32,
                    out.as_mut_ptr() as i32,
                    MAX_VALUE as i32,
                )
            };
            if rc < 0 {
                return Err(err_from_code(rc));
            }
            out.truncate(rc as usize);
            Ok(out)
        }

        fn spawn(&mut self, template_code_hash: &[u8; 32]) -> Result<ContractId, DexError> {
            let mut out = [0u8; 32];
            let rc = unsafe {
                ffi::dex_spawn(template_code_hash.as_ptr() as i32, out.as_mut_ptr() as i32)
            };
            if rc == 0 {
                Ok(ContractId(out))
            } else {
                Err(err_from_code(rc))
            }
        }

        fn consume_fuel(&mut self, units: u64) -> Result<(), DexError> {
            let rc = unsafe { ffi::dex_consume_fuel(units as i64) };
            if rc == 0 {
                Ok(())
            } else {
                Err(DexError::OutOfFuel)
            }
        }
    }

    /// Deterministic bump allocator: no external dependency, never frees.
    /// Suitable for one-shot contract calls (fresh instance per call).
    pub struct BumpAlloc {
        offset: UnsafeCell<usize>,
    }

    // SAFETY: wasm32 contract execution is single-threaded by construction
    // (fresh instance per call, no threads in the consensus profile).
    #[allow(unsafe_code)]
    unsafe impl Sync for BumpAlloc {}

    impl BumpAlloc {
        pub const fn new() -> Self {
            Self {
                offset: UnsafeCell::new(0),
            }
        }
    }

    impl Default for BumpAlloc {
        fn default() -> Self {
            Self::new()
        }
    }

    const HEAP_SIZE: usize = 1 << 20; // 1 MiB static heap
    static mut HEAP: [u8; HEAP_SIZE] = [0; HEAP_SIZE];

    // SAFETY: single-threaded bump allocation over a static buffer; the
    // offset cell is only touched here, alignment is enforced, and
    // exhaustion returns null (alloc error) instead of corrupting memory.
    #[allow(unsafe_code)]
    unsafe impl core::alloc::GlobalAlloc for BumpAlloc {
        unsafe fn alloc(&self, layout: core::alloc::Layout) -> *mut u8 {
            let offset = &mut *self.offset.get();
            let align = layout.align().max(1);
            let start = (*offset + align - 1) & !(align - 1);
            let end = match start.checked_add(layout.size()) {
                Some(end) if end <= HEAP_SIZE => end,
                _ => return core::ptr::null_mut(),
            };
            *offset = end;
            core::ptr::addr_of_mut!(HEAP).cast::<u8>().add(start)
        }

        unsafe fn dealloc(&self, _ptr: *mut u8, _layout: core::alloc::Layout) {}
    }

    /// Shared entry runner: read input, dispatch, write output.
    /// Returns 0 on success; the `DexError` code (positive) on failure,
    /// which the runtime treats as a deterministic revert.
    #[allow(unsafe_code)] // SAFETY: see `ffi` module note.
    pub fn run(
        opcode: i32,
        input_len: i32,
        dispatch: fn(&mut WasmHost, u16, &[u8]) -> Result<Vec<u8>, DexError>,
    ) -> i32 {
        let len = input_len.max(0) as usize;
        let mut input = alloc::vec![0u8; len];
        if len > 0 {
            let got = unsafe { ffi::input_read(input.as_mut_ptr() as i32, 0, input.len() as i32) };
            if got < 0 {
                return DexError::InvalidArguments.code() as i32;
            }
            input.truncate(got as usize);
        }
        let opcode = match u16::try_from(opcode) {
            Ok(op) => op,
            Err(_) => return DexError::InvalidOpcode.code() as i32,
        };
        let mut host = WasmHost;
        match dispatch(&mut host, opcode, &input) {
            Ok(output) => {
                let rc = unsafe { ffi::output_write(output.as_ptr() as i32, output.len() as i32) };
                if rc == 0 {
                    0
                } else {
                    DexError::InvalidArguments.code() as i32
                }
            }
            Err(err) => err.code() as i32,
        }
    }
}

/// Declare the wasm32 contract entry point for a DEX contract.
#[macro_export]
macro_rules! contract_entry {
    ($dispatch:path) => {
        #[cfg(target_arch = "wasm32")]
        mod __zalkanes_dex_entry {
            // No #[panic_handler] here: `zalkanes-core` links `std` into
            // the wasm build, whose abort handler (panic = "abort") is the
            // deterministic trap path. The bump allocator overrides std's
            // default allocator for deterministic allocation behavior.
            #[global_allocator]
            static ALLOC: $crate::wasm::BumpAlloc = $crate::wasm::BumpAlloc::new();

            #[no_mangle]
            pub extern "C" fn dispatch(opcode: i32, input_len: i32) -> i32 {
                $crate::wasm::run(opcode, input_len, $dispatch)
            }
        }
    };
}

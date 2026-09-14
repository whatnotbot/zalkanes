# Building contracts

## Toolchain

`rust-toolchain.toml` pins Rust 1.88.0 and adds the `wasm32-unknown-unknown`
target; `rustup` installs both the first time you run `cargo` in the
repository. No other tool is needed to produce a deployable module.

## The command

```bash
zalkanes contract build --manifest-path ./contracts/counter
```

`--manifest-path` accepts the crate directory or its `Cargo.toml`; it
defaults to `./contracts/counter`. The command runs

```text
cargo build --release --target wasm32-unknown-unknown --manifest-path <crate>/Cargo.toml
```

with `RUSTFLAGS="-C link-arg=-s"` (strip symbols) and `SOURCE_DATE_EPOCH=0`,
and prints the artifact:

```text
Built ./contracts/counter for wasm32-unknown-unknown
wasm: ./contracts/counter/target/wasm32-unknown-unknown/release/counter.wasm (2513 bytes)
```

Running `cargo build` directly with the same arguments produces the same
file; the CLI wrapper only pins the flags and reports the path. The artifact
name is the crate name with `-` replaced by `_` (`key-value` builds
`key_value.wasm`).

## Contract crate layout

Copy `contracts/counter` and rename it. The essentials:

- `crate-type = ["cdylib"]`, `#![no_std]`, `#![no_main]`.
- A `#[global_allocator]` (the examples use `wee_alloc`) and a
  `#[panic_handler]`.
- `panic = "abort"` in the release profile, so a Rust panic becomes a WASM
  trap rather than unwinding code that would not validate.
- `opt-level = "s"`, `lto = true`, `strip = true`, to keep the module small.
  Size matters: each 1400 bytes of WASM is one carrier input in the DEPLOY
  transaction and raises the ZIP-317 fee.
- An empty `[workspace]` table, so the contract is not a member of the
  root workspace (root `cargo test --workspace` stays host-only).
- No `std`, no floating point, no threads, no `wasm-bindgen`. See
  [wasm-rules.md](wasm-rules.md).

## Validating before you deploy

`zalkanes contract deploy` validates the module locally with the same
function every node uses (`zalkanes_runtime::validate_module`) before
spending anything, so an undeployable module fails fast:

```text
Error: WASM validation failed: unresolvable import env::contract_call (host ABI provides only env::{storage_get, storage_set, storage_delete, output_write, context_block_height, input_read})
```

`--dry-run` goes one step further and prints the funding plan without signing
or broadcasting.

## Reproducibility

Given the same toolchain, the same sources, and the flags above, the build is
byte-for-byte reproducible, and the node's release binaries are built the
same way in CI (`audit/BUILD-REPRODUCIBILITY.md`). The code hash printed by
`deploy` (SHA-256 of the file) is what ends up on chain; two developers who
build the same crate should see the same hash. `crates/zalkanes-build`
exposes the same build as a library function (`build_contract`) for tooling.

## Lockfiles

Each example contract has its own `Cargo.lock`. `cargo build` updates a stale
lockfile in place; if you see a lockfile change you did not intend after
building an example, discard it or commit it deliberately.

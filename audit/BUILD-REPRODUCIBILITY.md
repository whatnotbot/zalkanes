# Build reproducibility and release artifacts

## Pinned inputs

| input | pin |
|---|---|
| Rust toolchain | `rust-toolchain.toml` → channel `1.88.0`, components rustfmt/clippy/rust-src, targets `wasm32-unknown-unknown` + `x86_64-unknown-linux-gnu` |
| Dependencies | `Cargo.lock` committed; release builds use `--locked` |
| WASM engine | `wasmi` **=2.0.0** (any change is a protocol change) |
| Zcash stack | `zcash_primitives` =0.30.1, `orchard` =0.15.5, `pczt` =0.9.3, `zcash_client_sqlite` =0.22.0 |
| State store | `rocksdb` 0.22.0 |
| Base node | Zebra **6.3.0** |
| Protocol manifest | `protocol/v0.toml`, SHA-256 `57178628cebadad21da5e5c6495a7d646a55c0299609ea8939737744ab0b8752` |

The manifest hash is compiled into the binary (`include_str!` + SHA-256) and
exposed via `zalkanes_getInfo.protocol_manifest_hash`, so operators can detect
divergence without trusting a version string.

## Reproducibility procedure

The `release-engineering` workflow performs, per target:

1. **Two independent clean builds** of `zalkanes-cli` from the same commit, in
   separate directories with separate target directories and no shared cache.
2. Both under fixed `SOURCE_DATE_EPOCH=1700000000` and
   `--remap-path-prefix` for both the source tree and `$CARGO_HOME`, so build
   paths cannot leak into the binary.
3. A hard comparison of the two SHA-256 digests. **The job fails if they
   differ**, and dumps the first differing byte offsets so a divergence is
   diagnosed precisely rather than hand-waved.

Targets: `x86_64-unknown-linux-gnu` (ubuntu-24.04) and
`aarch64-unknown-linux-gnu` (ubuntu-24.04-arm, native).

> Path leakage is a known reproducibility hazard for Rust binaries — panic
> messages embed source paths. The remapping above is what makes two builds
> in different directories comparable.

## Artifacts

Per target the workflow publishes:

| artifact | contents |
|---|---|
| `zalkanes-<target>` | the release binary |
| `SHA256SUMS` | binary digest |
| `MANIFEST.txt` | source commit + ref, target triple, exact `rustc`/`cargo` versions, `SOURCE_DATE_EPOCH`, protocol manifest hash, reproducibility verdict, binary sha256 |
| `*.cdx.json` | CycloneDX SBOM (`cargo-cyclonedx`) |

Verify before installing:

```
sha256sum -c SHA256SUMS
cat MANIFEST.txt
```

## Contract builds

`scripts/build-reproducible.sh` builds the reference contracts with
`SOURCE_DATE_EPOCH=0` and `RUSTFLAGS=-C link-arg=-s`, printing each WASM's
SHA-256. The canonical counter contract used in live acceptance hashes to
`fa8289fbc0fdb132e57f939033a9971b0ee2990ec49db247aad3f682aa804cc7`
(2,513 bytes).

## Container

`Dockerfile` builds `zalkanes-cli` and copies `protocol/` into the image (the
manifest is compiled in via `include_str!`). Runtime configuration is
environment-driven; **no secrets are baked into the image**. Zebra images
(`deploy/zebra/`) build `zebrad` from the pinned `v6.3.0` git tag.

## Status

The reproducibility gate is **enforced in CI as of this commit**; consult the
`release-engineering` workflow run for the specific candidate to see the
recorded digests and verdict. Reproducibility is not claimed here beyond what
that job actually verified.

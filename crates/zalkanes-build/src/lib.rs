//! # zalkanes-build
//!
//! Reproducible WASM build helpers.

#![forbid(unsafe_code)]

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Build a contract crate for `wasm32-unknown-unknown` with reproducible flags.
///
/// Returns the path to the produced `.wasm` file.
pub fn build_contract(manifest_path: &Path) -> Result<PathBuf> {
    let output = Command::new("cargo")
        .args([
            "build",
            "--release",
            "--target",
            "wasm32-unknown-unknown",
            "--manifest-path",
            manifest_path.to_str().context("invalid manifest path")?,
        ])
        .env("RUSTFLAGS", "-C link-arg=-s")
        .env("SOURCE_DATE_EPOCH", "0")
        .output()
        .context("failed to run cargo build")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("cargo build failed:\n{stderr}");
    }

    // Find the .wasm in target/wasm32-unknown-unknown/release/
    let manifest_dir = manifest_path
        .parent()
        .context("manifest has no parent directory")?;
    let target_dir = manifest_dir.join("target/wasm32-unknown-unknown/release");

    let wasm = std::fs::read_dir(&target_dir)
        .with_context(|| format!("reading target dir {}", target_dir.display()))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .find(|p| p.extension().and_then(|e| e.to_str()) == Some("wasm"))
        .with_context(|| format!("no .wasm found in {}", target_dir.display()))?;

    Ok(wasm)
}

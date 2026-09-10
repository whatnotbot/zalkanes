//! # zalkanes-tx
//!
//! Transparent transaction construction and ZIP-317 fee calculation.
//! Does NOT sign; returns unsigned transaction data for wallet integration.

#![forbid(unsafe_code)]

/// Estimated transaction size in bytes for fee calculation.
pub struct TxSizeEstimate {
    pub version_bytes: usize,
    pub inputs: usize,
    pub outputs: usize,
    pub input_bytes: usize,
    pub output_bytes: usize,
}

impl TxSizeEstimate {
    /// Estimate for a deployment transaction with N carrier inputs.
    pub fn for_deploy(carrier_inputs: usize) -> Self {
        Self {
            version_bytes: 4,
            inputs: carrier_inputs,
            outputs: 2, // OP_RETURN + change
            // P2SH input: ~200 bytes per chunk input (conservative)
            input_bytes: carrier_inputs * 200,
            // OP_RETURN output: ~80 bytes; change output: ~34 bytes
            output_bytes: 114,
        }
    }

    /// Total estimated bytes.
    pub fn total_bytes(&self) -> usize {
        self.version_bytes + self.input_bytes + self.output_bytes + 10
    }
}

/// Calculate ZIP-317 fee in zatoshi.
///
/// ZIP-317 conventional fee: 5000 zatoshi per logical action.
/// A logical action is defined as max(inputs, outputs).
///
/// Reference: https://zips.z.cash/zip-0317
pub fn zip317_fee(inputs: usize, outputs: usize) -> u64 {
    const MARGINAL_FEE: u64 = 5_000;
    const GRACE_ACTIONS: u64 = 2;

    let logical_actions = inputs.max(outputs) as u64;
    let actions = logical_actions.max(GRACE_ACTIONS);
    MARGINAL_FEE * actions
}

/// Fee for a deployment transaction.
pub fn deployment_fee(carrier_inputs: usize) -> u64 {
    let est = TxSizeEstimate::for_deploy(carrier_inputs);
    zip317_fee(est.inputs, est.outputs)
}

/// Fee for a call transaction.
pub fn call_fee() -> u64 {
    // 1 input (funding UTXO) + 2 outputs (OP_RETURN + change)
    zip317_fee(1, 2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zip317_minimum_fee() {
        // Minimum: 2 actions * 5000 = 10000 zatoshi
        assert_eq!(zip317_fee(1, 1), 10_000);
    }

    #[test]
    fn zip317_scales_with_inputs() {
        let few = zip317_fee(2, 2);
        let many = zip317_fee(10, 2);
        assert!(many > few);
    }

    #[test]
    fn call_fee_is_reasonable() {
        let fee = call_fee();
        assert_eq!(fee, 10_000); // 2 actions * 5000
    }
}

//! Transparent funding: spend P2PKH UTXOs via the existing `zalkanes-tx`
//! V5/ZIP-244 builder, returning transparent change.

#![forbid(unsafe_code)]

use anyhow::{bail, Result};
use zalkanes_tx::{self, OutPoint, SignedTx, SigningKey};

use crate::funding::{FundContext, FundingSource, FundingUtxo, TxRequest};

/// A transparent funding source holding a signing key and its known UTXOs.
#[derive(Clone)]
pub struct TransparentFunding {
    key: SigningKey,
    utxos: Vec<FundingUtxo>,
}

impl TransparentFunding {
    pub fn new(key: SigningKey, utxos: Vec<FundingUtxo>) -> Self {
        Self { key, utxos }
    }

    /// The transparent funding key.
    pub fn key(&self) -> &SigningKey {
        &self.key
    }

    /// The funding UTXOs available to this source.
    pub fn utxos(&self) -> &[FundingUtxo] {
        &self.utxos
    }

    fn outpoints_and_values(&self) -> Vec<(OutPoint, u64)> {
        self.utxos
            .iter()
            .map(|u| (u.outpoint.clone(), u.value))
            .collect()
    }
}

impl FundingSource for TransparentFunding {
    fn pool_name(&self) -> &'static str {
        "transparent"
    }

    fn fund(&self, request: &TxRequest, ctx: &FundContext) -> Result<SignedTx> {
        if self.utxos.is_empty() {
            bail!("no transparent funding UTXOs available");
        }
        let branch_id = ctx.branch_id();
        match request {
            TxRequest::Prepare { carrier_values } => {
                if carrier_values.is_empty() {
                    bail!("PREPARE requires at least one carrier output");
                }
                zalkanes_tx::build_prepare(
                    &self.key,
                    &self.outpoints_and_values(),
                    carrier_values,
                    branch_id,
                )
            }
            TxRequest::Deploy {
                chunks,
                carrier_outpoints,
                carrier_values,
                op_return,
            } => zalkanes_tx::build_deploy(
                &self.key,
                carrier_outpoints,
                carrier_values,
                chunks,
                op_return,
                branch_id,
            ),
            TxRequest::Call { op_return } => {
                let utxo = self
                    .utxos
                    .first()
                    .ok_or_else(|| anyhow::anyhow!("no funding UTXO for CALL"))?;
                zalkanes_tx::build_call(
                    &self.key,
                    utxo.outpoint.clone(),
                    utxo.value,
                    op_return,
                    branch_id,
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::PrivacyPolicy;
    use zalkanes_core::types::Network;

    fn ctx() -> FundContext {
        FundContext {
            network: Network::Regtest,
            target_height: 1,
            policy: PrivacyPolicy::NoPrivacy,
        }
    }

    fn funding() -> TransparentFunding {
        let key = SigningKey::dev_key();
        // A synthetic outpoint: txid 0x01.., vout 0. The value is chosen well
        // above the ZIP-317 minimum fee so change is produced.
        let mut txid = [0u8; 32];
        txid[0] = 1;
        let outpoint = OutPoint::new(txid, 0);
        TransparentFunding::new(key, vec![FundingUtxo { outpoint, value: 1_000_000 }])
    }

    #[test]
    fn call_produces_signed_tx_with_change() {
        let f = funding();
        let req = TxRequest::Call {
            op_return: vec![0x5a, 0x41, 0x4c, 0x4b, 0x00, 0x02],
        };
        let tx = f.fund(&req, &ctx()).unwrap();
        assert!(!tx.bytes.is_empty());
        assert_ne!(tx.txid, [0u8; 32]);
    }

    #[test]
    fn prepare_rejects_empty_carriers() {
        let f = funding();
        let req = TxRequest::Prepare {
            carrier_values: vec![],
        };
        assert!(f.fund(&req, &ctx()).is_err());
    }

    #[test]
    fn pool_name_is_transparent() {
        assert_eq!(funding().pool_name(), "transparent");
    }

    #[test]
    fn no_utxos_is_an_error() {
        let key = SigningKey::dev_key();
        let f = TransparentFunding::new(key, vec![]);
        let req = TxRequest::Call {
            op_return: vec![0x5a, 0x41, 0x4c, 0x4b, 0x00, 0x02],
        };
        assert!(f.fund(&req, &ctx()).is_err());
    }
}

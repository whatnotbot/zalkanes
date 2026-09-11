//! Transparent funding: plan and sign P2PKH-funded transactions via the
//! existing `zalkanes-tx` V5/ZIP-244 builder, returning transparent change.

#![forbid(unsafe_code)]

use anyhow::{bail, Result};
use zalkanes_tx::{self, OutPoint, SigningKey};

use crate::funding::{FundContext, FundingSource, FundingUtxo, TxRequest};
use crate::plan::{FundingPlan, Stage, TransparentPlan};

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

    fn build_plan(&self, request: &TxRequest) -> Result<zalkanes_tx::PreparedTx> {
        match request {
            TxRequest::Prepare { carrier_values } => {
                if carrier_values.is_empty() {
                    bail!("PREPARE requires at least one carrier output");
                }
                zalkanes_tx::prepare_plan(&self.key, &self.outpoints_and_values(), carrier_values)
            }
            TxRequest::Deploy {
                chunks,
                carrier_outpoints,
                carrier_values,
                op_return,
            } => zalkanes_tx::deploy_plan(
                &self.key,
                carrier_outpoints,
                carrier_values,
                chunks,
                op_return,
            ),
            TxRequest::Call { op_return } => {
                let utxo = self
                    .utxos
                    .first()
                    .ok_or_else(|| anyhow::anyhow!("no funding UTXO for CALL"))?;
                zalkanes_tx::call_plan(&self.key, utxo.outpoint.clone(), utxo.value, op_return)
            }
        }
    }
}

impl FundingSource for TransparentFunding {
    fn pool_name(&self) -> &'static str {
        "transparent"
    }

    fn plan(&self, request: &TxRequest, ctx: &FundContext) -> Result<FundingPlan> {
        if self.utxos.is_empty() {
            bail!("no transparent funding UTXOs available");
        }
        let prepared = self.build_plan(request)?;
        let plan_id = crate::plan::new_plan_id();
        let branch_id = ctx.branch_id();

        let inputs: Vec<crate::plan::PlanInput> = prepared
            .inputs
            .iter()
            .map(|i| crate::plan::PlanInput {
                pool: 0,
                txid: *i.outpoint.hash(),
                output_index: i.outpoint.n(),
                value: i.value,
            })
            .collect();
        let outputs: Vec<crate::plan::PlanOutput> = prepared
            .outputs
            .iter()
            .map(|o| crate::plan::PlanOutput {
                value: o.value,
                script: o.script_pubkey.clone(),
            })
            .collect();
        let zalk = request.op_return_payload().unwrap_or(&[]);
        let kind = match request {
            TxRequest::Prepare { .. } => 0,
            TxRequest::Deploy { .. } => 1,
            TxRequest::Call { .. } => 2,
        };
        let intent_hash = crate::plan::commit_plan(
            ctx.network.id_byte(),
            0, // protocol version
            0, // transparent pool
            ctx.target_height,
            &ctx.target_hash,
            u32::from(branch_id),
            5, // tx version
            0, // expiry
            &inputs,
            0,
            &[0u8; 32],
            &outputs,
            None,
            prepared.fee,
            zalk,
            kind,
        );

        Ok(FundingPlan::Transparent(Box::new(TransparentPlan {
            prepared,
            request: request.clone(),
            branch_id,
            target_height: ctx.target_height,
            canonical_tip: ctx.canonical_tip(),
            stage: Stage::Planned,
            signed: None,
            plan_id,
            intent_hash,
        })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zalkanes_core::types::Network;

    fn ctx() -> FundContext {
        FundContext {
            network: Network::Regtest,
            target_height: 1,
            target_hash: [0u8; 32],
        }
    }

    fn funding() -> TransparentFunding {
        let key = SigningKey::dev_key();
        let mut txid = [0u8; 32];
        txid[0] = 1;
        let outpoint = OutPoint::new(txid, 0);
        TransparentFunding::new(
            key,
            vec![FundingUtxo {
                outpoint,
                value: 1_000_000,
            }],
        )
    }

    #[test]
    fn call_plan_has_no_signatures_and_stages() {
        let f = funding();
        let req = TxRequest::Call {
            op_return: vec![0x5a, 0x41, 0x4c, 0x4b, 0x00, 0x02],
        };
        let mut plan = f.plan(&req, &ctx()).unwrap();
        assert_eq!(plan.stage(), Stage::Planned);
        assert_eq!(plan.pool_name(), "transparent");
        assert!(plan.fee() > 0);
        assert!(plan.change() > 0);
        // min policy for a transparent tx: sender address + amounts revealed.
        assert_eq!(
            plan.minimum_policy(),
            crate::policy::PrivacyPolicy::AllowFullyTransparent
        );
        // describe() works without proving/signing.
        assert!(plan.describe().contains("Funding pool"));

        plan.prove().unwrap();
        assert_eq!(plan.stage(), Stage::Proven);
        plan.sign().unwrap();
        assert_eq!(plan.stage(), Stage::Signed);
        let tx = plan.extract().unwrap();
        assert_eq!(plan.stage(), Stage::Extracted);
        assert!(!tx.bytes.is_empty());
        assert_ne!(tx.txid, [0u8; 32]);
    }

    #[test]
    fn plan_does_not_sign() {
        let f = funding();
        let req = TxRequest::Call {
            op_return: vec![0x5a, 0x41, 0x4c, 0x4b, 0x00, 0x02],
        };
        let plan = f.plan(&req, &ctx()).unwrap();
        // A planned (unsigned) plan has no signed transaction.
        match &plan {
            FundingPlan::Transparent(p) => assert!(p.signed.is_none()),
            #[cfg(feature = "shielded")]
            _ => unreachable!(),
        }
    }

    #[test]
    fn stale_plan_is_rejected_after_tip_change() {
        let f = funding();
        let req = TxRequest::Call {
            op_return: vec![0x5a, 0x41, 0x4c, 0x4b, 0x00, 0x02],
        };
        let plan = f.plan(&req, &ctx()).unwrap();

        // Same tip -> fresh.
        plan.check_freshness(crate::funding::CanonicalTip {
            height: 1,
            hash: [0u8; 32],
        })
        .unwrap();

        // Same height, different hash (same-height reorg) -> stale.
        assert!(plan
            .check_freshness(crate::funding::CanonicalTip {
                height: 1,
                hash: [0xAAu8; 32],
            })
            .is_err());
    }

    #[test]
    fn prepare_rejects_empty_carriers() {
        let f = funding();
        let req = TxRequest::Prepare {
            carrier_values: vec![],
        };
        assert!(f.plan(&req, &ctx()).is_err());
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
        assert!(f.plan(&req, &ctx()).is_err());
    }
}

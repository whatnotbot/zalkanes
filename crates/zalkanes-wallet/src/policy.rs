//! Transaction privacy policy, mirroring Zallet's `z_sendmany` vocabulary.
//!
//! This vocabulary is about **value and address** privacy only. It is
//! deliberately kept distinct from the separate Zalkanes disclosure that
//! contract metadata (contract id, opcode, calldata) is always public on chain.
//!
//! The minimum policy a transaction requires is computed **per transaction** by
//! [`crate::plan::FundingPlan::minimum_policy`], not applied globally. For
//! example a fully-shielded CALL (shielded spend + zero-value OP_RETURN +
//! shielded change) requires `FullPrivacy`, while a shielded PREPARE (which
//! deshields value into transparent carrier outputs) requires
//! `AllowRevealedAmounts`.
//!
//! This is a UX/disclosure layer only — it does not change execution semantics
//! or any consensus rule.

#![forbid(unsafe_code)]

use std::{fmt, str::FromStr};

/// The most a transaction is permitted to reveal, ordered from most private to
/// least private (Zallet `z_sendmany` / `pczt_create` vocabulary).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PrivacyPolicy {
    /// Reveal nothing beyond what shielded pools already commit to.
    FullPrivacy,
    /// Shielded amounts may be revealed.
    AllowRevealedAmounts,
    /// Shielded recipient addresses may be revealed.
    AllowRevealedRecipients,
    /// Shielded sender addresses may be revealed.
    AllowRevealedSenders,
    /// The transaction may be fully transparent.
    AllowFullyTransparent,
    /// The transaction may link account addresses together.
    AllowLinkingAccountAddresses,
    /// No privacy constraints: reveal anything.
    NoPrivacy,
}

impl PrivacyPolicy {
    /// The Zallet JSON-RPC string for this policy.
    pub fn as_str(self) -> &'static str {
        match self {
            PrivacyPolicy::FullPrivacy => "FullPrivacy",
            PrivacyPolicy::AllowRevealedAmounts => "AllowRevealedAmounts",
            PrivacyPolicy::AllowRevealedRecipients => "AllowRevealedRecipients",
            PrivacyPolicy::AllowRevealedSenders => "AllowRevealedSenders",
            PrivacyPolicy::AllowFullyTransparent => "AllowFullyTransparent",
            PrivacyPolicy::AllowLinkingAccountAddresses => "AllowLinkingAccountAddresses",
            PrivacyPolicy::NoPrivacy => "NoPrivacy",
        }
    }

    /// Whether `self` permits everything `required` does (i.e. `self` is at
    /// least as permissive as `required`).
    pub fn permits(self, required: Self) -> bool {
        self >= required
    }

    /// A one-line description of what this policy allows, for CLI display.
    pub fn describe(self) -> &'static str {
        match self {
            PrivacyPolicy::FullPrivacy => "no additional information revealed",
            PrivacyPolicy::AllowRevealedAmounts => "amounts may be revealed",
            PrivacyPolicy::AllowRevealedRecipients => "shielded recipients may be revealed",
            PrivacyPolicy::AllowRevealedSenders => "shielded senders may be revealed",
            PrivacyPolicy::AllowFullyTransparent => "fully transparent transaction allowed",
            PrivacyPolicy::AllowLinkingAccountAddresses => {
                "account addresses may be linked together"
            }
            PrivacyPolicy::NoPrivacy => "no privacy constraints",
        }
    }
}

impl fmt::Display for PrivacyPolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for PrivacyPolicy {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "FullPrivacy" => Ok(PrivacyPolicy::FullPrivacy),
            "AllowRevealedAmounts" => Ok(PrivacyPolicy::AllowRevealedAmounts),
            "AllowRevealedRecipients" => Ok(PrivacyPolicy::AllowRevealedRecipients),
            "AllowRevealedSenders" => Ok(PrivacyPolicy::AllowRevealedSenders),
            "AllowFullyTransparent" => Ok(PrivacyPolicy::AllowFullyTransparent),
            "AllowLinkingAccountAddresses" => Ok(PrivacyPolicy::AllowLinkingAccountAddresses),
            "NoPrivacy" => Ok(PrivacyPolicy::NoPrivacy),
            other => Err(format!(
                "unknown privacy policy {other:?}; expected one of FullPrivacy, \
                 AllowRevealedAmounts, AllowRevealedRecipients, AllowRevealedSenders, \
                 AllowFullyTransparent, AllowLinkingAccountAddresses, NoPrivacy"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordering_is_monotonic() {
        let order = [
            PrivacyPolicy::FullPrivacy,
            PrivacyPolicy::AllowRevealedAmounts,
            PrivacyPolicy::AllowRevealedRecipients,
            PrivacyPolicy::AllowRevealedSenders,
            PrivacyPolicy::AllowFullyTransparent,
            PrivacyPolicy::AllowLinkingAccountAddresses,
            PrivacyPolicy::NoPrivacy,
        ];
        for w in order.windows(2) {
            assert!(w[1] > w[0], "{:?} should permit more than {:?}", w[1], w[0]);
        }
    }

    #[test]
    fn round_trips_str() {
        for p in [
            PrivacyPolicy::FullPrivacy,
            PrivacyPolicy::AllowRevealedAmounts,
            PrivacyPolicy::AllowRevealedRecipients,
            PrivacyPolicy::AllowRevealedSenders,
            PrivacyPolicy::AllowFullyTransparent,
            PrivacyPolicy::AllowLinkingAccountAddresses,
            PrivacyPolicy::NoPrivacy,
        ] {
            assert_eq!(p.as_str().parse::<PrivacyPolicy>().unwrap(), p);
        }
        assert!("Bogus".parse::<PrivacyPolicy>().is_err());
    }
}

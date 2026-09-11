# Testnet Evidence

Canonical: `docs/release/testnet-activation.md` (Milestone 2 acceptance).

Summary of the public-testnet acceptance evidence (V4-era, pre-freeze
experimental):

- Activation: height 4,338,100, block hash
  `000036528e38f12ab5ab5dba462f966496178a4a2138ff4f9acc6fb09d3e8098`, Nu6.3.
- Funding: 0.1 TAZ to `tmHvTQRfAw5uKq1j4M8gZH4JQJ3J6XWmi9X`.
- DEPLOY counter.wasm → contract `607a6246…`; 2 CALLs → `get()==2`;
  restart persistence + fresh reindex produced the identical state root.

> Note: Milestone 3 migrates to V5/ZIP-244 and freezes the wire format. The
> existing testnet deployment is **pre-freeze experimental**. A fresh frozen-RC
> testnet deployment is required before the milestone is complete.

## Update 2026-09-12: frozen-RC live acceptance supersedes the V4-era caveat

The full shielded-wallet live acceptance ran against the FROZEN protocol
(V5/ZIP-244 era manifest `57178628…`, verified live): shielded CALLs on the
original counter (2→3→4), byte-equality of transparent vs shielded ZALK
payloads, a fresh shielded-funded PREPARE → transparent DEPLOY (contract
`0af32d54…`, 0→1→2), and independent two-node root equality at height
4,339,534. Complete records: `live-acceptance-evidence.md`.

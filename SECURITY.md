# Security Policy

## Reporting a vulnerability

**Do not open a public GitHub issue for security vulnerabilities.**

Email: security@zalkanes.dev  (placeholder — replace before testnet launch)

Include:
- Description of the vulnerability
- Affected component(s) and version
- Steps to reproduce
- Your assessment of impact
- Any suggested mitigation

We will acknowledge within 72 hours and aim to ship a patch within 14 days for
critical issues.

## Scope

All consensus-critical crates are in scope:

- `zalkanes-protocol` (parser)
- `zalkanes-carrier` (P2SH carrier decoding)
- `zalkanes-runtime` (WASM execution)
- `zalkanes-state` (state engine / state root)
- `zalkanes-indexer` (block processing / reorg)
- `zalkanes-chain` (Zebra RPC adapter)

Out of scope for this policy (pre-audit v0):
- Mainnet deployment (no activation height is set)
- Non-consensus RPC/CLI behavior

## Threat model

See `docs/threat-model/` for the full threat model covering:
- Malicious contract authors
- Malicious transaction authors
- Malicious miners
- Chain reorganizations
- Software upgrades

## Audit status

**v0 has NOT been externally audited. Do not use on mainnet.**

Required before mainnet:
- WASM runtime audit
- Transaction carrier review
- Protocol specification review
- Fuzzing campaign completion
- Public testnet soak
